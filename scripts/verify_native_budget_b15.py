#!/usr/bin/env python3
"""B15a exact limits through CLI, independent hash and persisted receipts."""
import json,tempfile
from pathlib import Path
from verify_native_index_b9 import run,sha
def main():
    with tempfile.TemporaryDirectory(prefix="sedravasql-b15-") as t:
        root=Path(t);log=root/"log";probe=root/"probe";ops=root/"ops"
        run("init",log);run("init",probe)
        ops.write_text(json.dumps([dict(namespace="n",key="a",expected_revision=0,change={"kind":"Put","value":"привет"})]))
        key="b15-cli-request-1"
        first=run("batch",probe,key,ops);size=probe.stat().st_size
        before=sha(log);run("batch",log,key,ops,str(size-1),"10",fail=True);assert sha(log)==before
        assert run("batch",log,key,ops,str(size),"10")==first
        u=run("usage",log,str(size),"1")
        assert u["log_bytes"]==size and u["transactions"]==1 and u["receipts"]==1 and u["remaining_bytes"]==u["remaining_transactions"]==0
        before=sha(log);assert run("batch",log,key,ops,"8","1")["replayed"]
        ops.write_text(json.dumps([dict(namespace="n",key="a",expected_revision=1,change={"kind":"Put","value":"new"})]))
        run("batch",log,"b15-cli-request-2",ops,str(size),"10",fail=True)
        run("batch",log,"b15-cli-request-2",ops,"67108864","1",fail=True)
        assert sha(log)==before and run("receipt",log,"b15-cli-request-2") is None
        assert run("read",log,"n","a")["value"]=="привет"
        for x,y in [("7","1"),("67108865","1"),("8","0"),("8","65537")]:run("usage",log,x,y,fail=True)
        print("PASS B15a: byte-exact limit, transaction ceiling, restart and replay, invalid limits, unchanged SHA on rejection")
if __name__=="__main__":main()
