#!/usr/bin/env python3
"""Exercise range CLI with namespace boundaries, tombstones and source preservation."""
import json, tempfile
from pathlib import Path
from verify_native_index_b9 import run, sha

def main():
    with tempfile.TemporaryDirectory(prefix="sedravasql-b10-") as t:
        folder=Path(t)
        log=folder/"journal"; snap=folder/"snapshot"; index=folder/"index"; ops=folder/"ops.json"
        run("init",log)
        values=[("a","k01"),("a","k02"),("a","k03"),("b","k01"),("b","k03")]
        ops.write_text(json.dumps([dict(namespace=n,key=k,expected_revision=0,change={"kind":"Put","value":n+k}) for n,k in values]))
        run("batch",log,"b10-cli-initial",ops)
        ops.write_text(json.dumps([dict(namespace="a",key="k02",expected_revision=1,change={"kind":"Delete"})]))
        run("batch",log,"b10-cli-delete1",ops)
        before=sha(log)
        run("snapshot-create",log,snap,before)
        s=sha(snap)
        report=run("index-create",snap,index,s)
        digest=sha(index)
        out=run("index-range",index,"a","k01","k04","256")
        assert [(r["namespace"],r["key"],r["value"],r["revision"]) for r in out["rows"]]==[("a","k01","ak01",1),("a","k02",None,2),("a","k03","ak03",1)]
        assert out["sequence"]==2 and out["index_sha256"]==report["index_sha256"]
        assert run("index-range",index,"a","k01","k03","1")["rows"]==out["rows"][:1]
        assert [r["key"] for r in run("index-range",index,"b","k01","k03","256")["rows"]]==["k01"]
        empty=run("index-range",index,"missing","a","z","1")
        assert empty["rows"]==[] and empty["row_bytes_read"]==0
        for n,a,b,l in [("a","z","a","1"),("a","a","a","1"),("a","a","z","0"),("a","a","z","257"),("../bad","a","z","1")]:
            run("index-range",index,n,a,b,l,fail=True)
        assert (sha(log),sha(snap),sha(index))==(before,s,digest)
        print("PASS B10 CLI: sorted half-open ranges, namespace isolation, tombstones, limits, zero-row reads, source hashes")
if __name__=="__main__":main()
