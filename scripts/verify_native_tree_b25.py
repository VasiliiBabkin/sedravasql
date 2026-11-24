#!/usr/bin/env python3
import json,os,subprocess,tempfile
from pathlib import Path
from verify_native_index_b9 import BIN,run,sha
def main():
 with tempfile.TemporaryDirectory(prefix="sedravasql-b25-") as t:
    d=Path(t);db=d/"tree";v=d/"value.json";v.write_text(json.dumps("value"))
    run("tree-init",db,128)
    for i in range(8):run("tree-put",db,f"k{i:03}",0,v)
    assert run("tree-verify",db)["height"]==1
    # Ninth key splits the root leaf into two leaves and a branch.
    child=subprocess.Popen([str(BIN),"tree-put",str(db),"k008","0",str(v)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,env={**os.environ,"SEDRAVASQL_NATIVE_TEST_FAULT":"pager_after_first_page"})
    out,err=child.communicate(timeout=20);assert child.returncode==86,(out,err)
    lock=Path(str(db)+".lock");assert lock.read_text().strip()==f"pid={child.pid}";lock.unlink()
    report=run("tree-verify",db);assert report["height"]==2 and report["rows"]==9 and report["nodes"]==3
    assert not Path(str(db)+".wal").exists()
    rows=run("tree-range",db,"k000","k009",256);assert [r["key"] for r in rows]==[f"k{i:03}" for i in range(9)]
    before=sha(db);run("tree-put",db,"k008",0,v,fail=True);assert sha(db)==before
    print("PASS B25: root split interrupted after first page, WAL recovery, 9 ordered rows, 3 nodes, conflict unchanged")
if __name__=="__main__":main()
