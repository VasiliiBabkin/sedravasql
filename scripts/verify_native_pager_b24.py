#!/usr/bin/env python3
import json,os,subprocess,tempfile
from pathlib import Path
from verify_native_index_b9 import BIN,run,sha
def main():
 with tempfile.TemporaryDirectory(prefix="sedravasql-b24-") as t:
    d=Path(t);changes=d/"changes.json"
    changes.write_text(json.dumps([{"namespace":"n","key":"a","expected_revision":0,"value":"first"},{"namespace":"n","key":"b","expected_revision":0,"value":"second"}]))
    for point in ("pager_after_wal","pager_after_first_page","pager_after_data"):
        db=d/point;run("pager-init",db,32)
        before=db.read_bytes()
        child=subprocess.Popen([str(BIN),"pager-commit",str(db),str(changes)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,env={**os.environ,"SEDRAVASQL_NATIVE_TEST_FAULT":point})
        out,err=child.communicate(timeout=20)
        assert child.returncode==86,(point,out,err)
        lock=Path(str(db)+".lock");assert lock.read_text().strip()==f"pid={child.pid}";lock.unlink()
        wal=Path(str(db)+".wal");assert wal.exists()
        assert run("pager-verify",db)["generation"]==1
        assert not wal.exists()
        assert run("pager-get",db,"n","a")["value"]=="first"
        assert run("pager-get",db,"n","b")["value"]=="second"
        after=db.read_bytes()
        assert sum(before[i:i+4096]!=after[i:i+4096] for i in range(0,len(before),4096))==3
        assert run("pager-verify",db)["generation"]==1
    db=d/"damaged";run("pager-init",db,32)
    child=subprocess.Popen([str(BIN),"pager-commit",str(db),str(changes)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,env={**os.environ,"SEDRAVASQL_NATIVE_TEST_FAULT":"pager_after_wal"})
    out,err=child.communicate(timeout=20);assert child.returncode==86
    lock=Path(str(db)+".lock");assert lock.read_text().strip()==f"pid={child.pid}";lock.unlink()
    wal=Path(str(db)+".wal");b=bytearray(wal.read_bytes());b[-1]^=1;wal.write_bytes(b)
    before=sha(db);run("pager-verify",db,fail=True);assert sha(db)==before and wal.exists()
 print("PASS B24: 3 controlled WAL crash points, 2 rows recovered atomically, 3 changed pages per commit, corrupted WAL rejected unchanged")
if __name__=="__main__":main()
