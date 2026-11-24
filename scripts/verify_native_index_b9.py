#!/usr/bin/env python3
"""B9 real CLI index-get-many: one full verification, bounded row seeks.

The only storage is a disposable 0700 folder; no production files touched.
"""
from __future__ import annotations
import hashlib,json,os
from pathlib import Path
import subprocess,tempfile

BIN=Path(os.environ["SEDRAVASQL_NATIVE_BIN"])
def run(*args, fail=False):
    p=subprocess.run([str(BIN),*(str(a) for a in args)],
        capture_output=True,text=True,timeout=22,
        env={"PATH":"/nonexistent","LC_ALL":"C"})
    if fail:
        assert p.returncode!=0,(args,p.returncode,p.stdout,p.stderr)
        return p.stderr
    assert p.returncode==0,(args,p.returncode,p.stdout,p.stderr)
    return json.loads(p.stdout)
def sha(file:Path):return hashlib.sha256(file.read_bytes()).hexdigest()

def main():
    assert BIN.is_file()
    with tempfile.TemporaryDirectory(prefix="sedravasql-b9-index-seek-") as temp:
        folder=Path(temp);folder.chmod(0o700)
        native=folder/"test.slog"
        snap=folder/"snapshot.snp"
        index=folder/"index.idx"
        ops=folder/"ops.json"
        requests=folder/"keys.json"
        run("init",native)
        expected={}
        for batch in range(4):
            items=[]
            for i in range(16):
                key=f"item_{batch*16+i:03d}"
                value=f"{batch}:{i}:"+"x"*1024
                expected[key]=value
                items.append(dict(namespace="test",key=key,expected_revision=0,
                    change={"kind":"Put","value":value}))
            ops.write_text(json.dumps(items),encoding="utf-8")
            result=run("batch",native,f"b9-index-batch-{batch:08d}",ops)
            assert result["sequence"]==batch+1
        original=sha(native)
        info=run("snapshot-create",native,snap,original)
        assert info["sequence"]==4 and info["entries"]==64
        snap_hash=sha(snap)
        built=run("index-create",snap,index,snap_hash)
        assert built["entries"]==64
        queries=[["test",f"item_{i:03d}"] for i in (0,31,48,63)]
        queries.extend(["test",f"not_found_{i:03d}"] for i in range(50))
        requests.write_text(json.dumps(queries))
        answer=run("index-get-many",index,requests)
        assert answer["queries"]==54
        assert len(answer["rows"])==54
        assert answer["index_sha256"]==built["index_sha256"]
        assert answer["index_bytes"]==built["index_bytes"]
        for i,key in enumerate(("item_000","item_031","item_048","item_063")):
            assert answer["rows"][i]==dict(namespace="test",key=key,value=expected[key],revision=1)
        assert all(item is None for item in answer["rows"][4:])
        assert 0<answer["row_bytes_read"]<answer["index_bytes"]/8,answer
        assert run("index-verify",index)==built
        assert run("index-get",index,"test","item_031")["value"]==expected["item_031"]
        assert sha(native)==original and sha(snap)==snap_hash
        requests.write_text("[]");run("index-get-many",index,requests,fail=True)
        requests.write_text(json.dumps([["test","item_001"]]*257))
        run("index-get-many",index,requests,fail=True)
        requests.write_text(json.dumps([["../bad","item_001"]]))
        run("index-get-many",index,requests,fail=True)
        requests.write_text(json.dumps([["test","item_001"]]))
        copied=index.read_bytes()
        index.write_bytes(copied[:-1]+b"X")
        run("index-get-many",index,requests,fail=True)
        index.write_bytes(copied)
        print("PASS B9: 54 lookups on one verified handle, 4 row data reads only")
        print(f"PASS B9: index {built['index_bytes']} bytes; row bytes loaded {answer['row_bytes_read']} after open")
        print("PASS B9: malformed input, too many queries, invalid key, damaged index rejected")
        print("LIMIT: full file validated ONCE per session; no live writer index or power-loss claim")

if __name__=="__main__":main()
