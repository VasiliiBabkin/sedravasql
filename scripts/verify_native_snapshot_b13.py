#!/usr/bin/env python3
"""B13 CLI: coherent single-generation live journal reads and historical receipts."""
import json,tempfile
from pathlib import Path
from verify_native_index_b9 import run,sha
def main():
    with tempfile.TemporaryDirectory(prefix="sedravasql-b13-") as t:
        root=Path(t);log=root/"journal";ops=root/"ops";q=root/"queries"
        run("init",log)
        q.write_text(json.dumps([["n","b"],["n","a"],["n","missing"],["n","b"]]))
        assert run("read-many",log,q)=={"sequence":0,"rows":[None]*4}
        ops.write_text(json.dumps([dict(namespace="n",key=k,expected_revision=0,change={"kind":"Put","value":k}) for k in ["a","b"]]))
        first=run("batch",log,"b13-cli-request-1",ops)
        view=run("read-many",log,q); assert view["sequence"]==1
        assert [r["value"] if r else None for r in view["rows"]]==["b","a",None,"b"]
        ops.write_text(json.dumps([dict(namespace="n",key="a",expected_revision=1,change={"kind":"Put","value":"new"}),dict(namespace="n",key="b",expected_revision=0,change={"kind":"Delete"})]))
        before=sha(log);run("batch",log,"b13-cli-rejected1",ops,fail=True)
        assert run("read-many",log,q)==view and sha(log)==before
        ops.write_text(json.dumps([dict(namespace="n",key="a",expected_revision=1,change={"kind":"Put","value":"new"}),dict(namespace="n",key="b",expected_revision=1,change={"kind":"Delete"})]))
        run("batch",log,"b13-cli-request-2",ops)
        before=sha(log);v=run("read-many",log,q)
        assert v["sequence"]==2 and v["rows"][0]["value"] is None and v["rows"][0]["revision"]==2
        assert v["rows"][1]["value"]=="new" and v["rows"][0]==v["rows"][3]
        assert run("receipt",log,"b13-cli-request-1")==first
        assert run("receipt",log,"b13-cli-rejected1") is None
        for keys in [[],[["n","a"]]*17,[["n","a"],["n","../bad"]]]:
            q.write_text(json.dumps(keys));run("read-many",log,q,fail=True)
        assert sha(log)==before
        print("PASS B13: coherent generations, order, missing/tombstone distinction, rejected CAS atomicity, durable historical receipt, bounds")
if __name__=="__main__":main()
