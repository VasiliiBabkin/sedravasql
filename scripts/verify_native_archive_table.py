#!/usr/bin/env python3
"""Independent CLI roundtrip for typed multi-table rows and lossless archive."""
import json,tempfile,hashlib,zlib
from pathlib import Path
from verify_native_index_b9 import run,sha
def main():
    with tempfile.TemporaryDirectory(prefix="sedravasql-archive-table-") as t:
        root=Path(t);log=root/"table.log";arc=root/"archive";restored=root/"restored";input=root/"input.json"
        run("init",log)
        def write(x):input.write_text(json.dumps(x,ensure_ascii=False));return input
        def schema(table):return dict(name=table,primary_key="id",columns=[dict(name="id",kind="text"),dict(name="amount",kind="int"),dict(name="note",kind="text")])
        def put(table,key,rev,amount):return dict(kind="put",table=table,expected_revision=rev,row=dict(id=key,amount=amount,note="повторяемый текст "*30))
        for name in ["Orders","Payments"]:
            run("table-create",log,"schema-"+name+"-001",write(schema(name)))
            assert run("table-schema",log,name)==schema(name)
        changes=[put("Orders","a",0,100),put("Payments","a",0,100)]
        first=run("table-batch",log,"table-batch-000001",write(changes))
        for i in range(1,33):
            run("table-batch",log,f"table-batch-{i+1:06}",write([put("Orders","a",i,100+i),put("Payments","a",i,100+i)]))
        before=sha(log)
        run("table-batch",log,"table-batch-invalid1",write([put("Orders","a",33,999),put("Payments","a",0,999)]),fail=True)
        invalid=put("Orders","b",0,1);invalid["row"]["amount"]="1"
        run("table-batch",log,"table-batch-invalid2",write([put("Orders","a",33,999),invalid]),fail=True)
        assert sha(log)==before
        assert run("table-get",log,"Orders","a")["values"]["amount"]==132
        assert run("table-get",log,"Orders","b") is None
        run("table-put",log,"table-row-000034","Orders","0",write(put("Orders","b",0,2)["row"]))
        assert [r["key"] for r in run("table-scan",log,"Orders","2")]==["a","b"]
        assert [r["key"] for r in run("table-scan",log,"Orders","2","a")]==["b"]
        run("table-delete",log,"table-delete-0001","Orders","b","1")
        assert run("table-get",log,"Orders","b")["values"] is None
        original=log.read_bytes();digest=sha(log)
        report=run("archive-create",log,arc,digest)
        assert report==run("archive-verify",arc)
        packed=arc.read_bytes();assert packed[:8]==b"SEDRARC1"
        assert hashlib.sha256(packed[:-32]).digest()==packed[-32:]
        assert zlib.decompress(packed[56:-32])==original
        assert report["archive_bytes"]<report["source_bytes"]
        assert run("archive-restore",arc,restored,sha(arc))==report and restored.read_bytes()==original
        assert run("table-batch",restored,"table-batch-000001",write(changes))["replayed"]
        assert run("receipt",restored,"table-batch-000001")==first
        assert run("table-get",restored,"Orders","a")["values"]["amount"]==132
        assert run("table-get",restored,"Orders","b")["values"] is None
        run("archive-restore",arc,restored,sha(arc),fail=True)
        assert sha(log)==digest==sha(restored)
        # Recompute outer checksum after removing compressed trailer: invalid stream.
        damaged=root/"damaged";prefix=packed[:-33];damaged.write_bytes(prefix+hashlib.sha256(prefix).digest());damaged.chmod(0o600)
        run("archive-verify",damaged,fail=True)
        print(f"PASS B15b/B16a CLI: typed cross-table atomic writes, tombstones, ordered scan, exact archive restore, preserved historical receipt; journal={len(original)} archive={len(packed)} bytes")
if __name__=="__main__":main()
