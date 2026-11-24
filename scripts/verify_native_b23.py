#!/usr/bin/env python3
"""B23 actual child-process sequential transaction and COW page refresh crash tests."""
import json,os,subprocess,tempfile
from pathlib import Path
from verify_native_index_b9 import BIN,run,sha
def main():
 with tempfile.TemporaryDirectory(prefix="sedravasql-b23-") as t:
    f=Path(t);db=f/"log";tx=f/"tx.json";image=f/"image";stage=f/"stage"
    run("init",db);run("sql",db,"CREATE TABLE T (id TEXT PRIMARY KEY,n INT)","b23-schema-0001")
    run("page-build-log",db,image);old=sha(image)
    tx.write_text(json.dumps([{"sql":"INSERT INTO T (id,n) VALUES (?,?)","params":["a",1]},{"sql":"SELECT n FROM T WHERE id=?","params":["a"]},{"sql":"UPDATE T SET n=? WHERE id=? AND n=?","params":[2,"a",1]},{"sql":"SELECT n FROM T WHERE id=?","params":["a"]}]))
    first=run("sql-sequential",db,"b23-sequence-0001",tx)
    assert first["affected"]==1 and [r["rows"] for r in first["reads"]]==[[[1]],[[2]]]
    before=sha(db)
    assert run("sql-sequential",db,"b23-sequence-0001",tx)=={**first,"replayed":True}
    tx.write_text(json.dumps([{"sql":"UPDATE T SET n=3 WHERE id='a'"},{"sql":"UPDATE T SET n=4 WHERE id='a' AND n=2"}]))
    run("sql-sequential",db,"b23-sequence-0002",tx,fail=True)
    assert sha(db)==before
    assert not run("page-match-log",image,db)["matches"]
    def crash(point):
        env={**os.environ,"SEDRAVASQL_NATIVE_TEST_FAULT":point}
        child=subprocess.Popen([str(BIN),"page-refresh",str(db),str(image),str(stage),sha(image)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,env=env)
        out,err=child.communicate(timeout=20)
        assert child.returncode==86,(child.returncode,out,err)
        # Stale lock removal is restricted to the exact dead child in this disposable test.
        lock=Path(str(db)+".lock")
        assert lock.is_file() and lock.read_text().strip()==f"pid={child.pid}",(lock.read_text(),child.pid)
        lock.unlink()
        assert sha(db)==before
        run("verify",db)
    crash("page_before_rename")
    assert sha(image)==old and stage.exists()
    assert run("page-match-log",stage,db)["matches"]
    assert not run("page-match-log",image,db)["matches"]
    stage.unlink()
    crash("page_after_rename")
    assert not stage.exists() and sha(image)!=old
    assert run("page-match-log",image,db)["matches"]
    assert run("page-get",image,"sedravasql_table.T","a")["row"]["value"]
    assert sha(db)==before
    print("PASS B23: sequential bound SQL, intermediate reads, exact replay, unchanged journal on rejection; 2 controlled refresh process exits, valid old/new image and lineage")
if __name__=="__main__":main()
