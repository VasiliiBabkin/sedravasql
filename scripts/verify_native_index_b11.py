#!/usr/bin/env python3
"""B11 CLI: one explicit row seek for a contiguous verified range."""
import json, tempfile
from pathlib import Path
from verify_native_index_b9 import run, sha

def main():
    with tempfile.TemporaryDirectory(prefix="sedravasql-b11-") as t:
        root=Path(t); log=root/"log"; snap=root/"snap"; idx=root/"idx"; ops=root/"ops.json"
        run("init",log)
        for batch in range(16):
            ops.write_text(json.dumps([dict(namespace="test", key=f"k{batch*16+i:03}", expected_revision=0,change={"kind":"Put","value":str(batch*16+i)}) for i in range(16)]))
            run("batch",log,f"b11-cli-batch-{batch:04}",ops)
        before=sha(log); run("snapshot-create",log,snap,before)
        snap_hash=sha(snap); run("index-create",snap,idx,snap_hash); idx_hash=sha(idx)
        for start,end,limit in [(0,256,256),(31,72,256),(31,72,3),(255,256,1),(256,257,256)]:
            out=run("index-range",idx,"test",f"k{start:03}",f"k{end:03}",str(limit))
            expected=list(range(start,min(end,start+limit,256)))
            assert [r["key"] for r in out["rows"]]==[f"k{i:03}" for i in expected]
            assert [r["value"] for r in out["rows"]]==list(map(str,expected))
            assert out["row_seeks"]==int(bool(expected))
            assert (out["row_bytes_read"]>0)==bool(expected)
        assert (sha(log),sha(snap),sha(idx))==(before,snap_hash,idx_hash)
        print("PASS B11: 256 rows / 1 explicit seek after open; bounded subranges and empty ranges; source hashes unchanged")
if __name__=="__main__":main()
