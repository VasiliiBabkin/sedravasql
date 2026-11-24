#!/usr/bin/env python3
"""Independent CLI pagination across process restarts, including tombstones."""
import json,tempfile
from pathlib import Path
from verify_native_index_b9 import run,sha
def main():
    with tempfile.TemporaryDirectory(prefix="sedravasql-b12-") as t:
        root=Path(t);log=root/"log";snap=root/"snap";idx=root/"idx";ops=root/"ops.json"
        run("init",log)
        for b in range(3):
            ops.write_text(json.dumps([dict(namespace="test",key=f"k{b*16+i:03}",expected_revision=0,change={"kind":"Put","value":str(b*16+i)}) for i in range(16)]))
            run("batch",log,f"b12-cli-batch-{b:04}",ops)
        ops.write_text(json.dumps([dict(namespace="test",key="k020",expected_revision=1,change={"kind":"Delete"})]))
        run("batch",log,"b12-cli-delete1",ops)
        before=sha(log);run("snapshot-create",log,snap,before);snap_hash=sha(snap)
        run("index-create",snap,idx,snap_hash);digest=sha(idx)
        base=("index-page",idx,"test","k003","k044")
        rows=[];cursor=None;pages=0;first=None
        while True:
            out=run(*base,"5",*([json.dumps(cursor)] if cursor else []))
            assert out["row_seeks"]==1
            rows.extend(out["rows"]);pages+=1;cursor=out["next"]
            if pages==1:first=cursor
            if cursor is None:break
        assert pages==9 and [x["key"] for x in rows]==[f"k{i:03}" for i in range(3,44)]
        assert rows[17]["value"] is None and rows[17]["revision"]==2
        assert run(*base,"256",json.dumps(first))["rows"]==rows[5:]
        for change in [{"index_sha256":"0"*64},{"namespace":"other"},{"start":"k004"},{"end":"k045"},{"after":"k009x"},{"unknown":1}]:
            run(*base,"5",json.dumps(first|change),fail=True)
        assert run("index-page",idx,"absent","a","z","1")["next"] is None
        # Different verified generation must not accept previous cursor.
        ops.write_text(json.dumps([dict(namespace="test",key="k001",expected_revision=1,change={"kind":"Put","value":"changed"})]))
        run("batch",log,"b12-cli-change1",ops)
        snap2=root/"snap2";idx2=root/"idx2"
        run("snapshot-create",log,snap2,sha(log));run("index-create",snap2,idx2,sha(snap2))
        run("index-page",idx2,"test","k003","k044","5",json.dumps(first),fail=True)
        assert sha(snap)==snap_hash and sha(idx)==digest
        print("PASS B12: 41 rows / 9 pages, no gaps or duplicates; tombstones; cursor scope and stale-generation rejection")
if __name__=="__main__":main()
