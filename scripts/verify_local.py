#!/usr/bin/env python3
"""SedravaSQL v0.1 independently-tested native CLI, concurrency and crash windows.
Debug fault injection is only used in isolated subprocesses in temporary dirs.
No production files, network listeners, OpenAI/GitHub CI or external accounts.
"""
from __future__ import annotations
import concurrent.futures
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]
BIN=ROOT/"target/debug/sedravasql"
def run(*args,ok=True,env=None,exitcode=None):
    local={"PATH":"/nonexistent","LC_ALL":"C"}
    if env:local.update(env)
    result=subprocess.run([str(BIN),*[str(a) for a in args]],cwd=ROOT,
            text=True,capture_output=True,timeout=20,env=local)
    if exitcode is not None:assert result.returncode==exitcode,(args,result.stderr)
    else:assert (result.returncode==0)==ok,(args,result.returncode,result.stderr)
    return json.loads(result.stdout) if result.returncode==0 else result
def check_sql(db):
    conn=sqlite3.connect(db)
    try:
        assert conn.execute("PRAGMA integrity_check").fetchone()[0]=="ok"
        assert conn.execute("PRAGMA user_version").fetchone()[0]==1
        assert conn.execute("SELECT count(*) FROM audit").fetchone()[0]==conn.execute(
            "SELECT count(*) FROM receipts").fetchone()[0]
        assert conn.execute("SELECT count(*) FROM audit a LEFT JOIN receipts r ON a.id=r.audit_id WHERE r.audit_id IS NULL").fetchone()[0]==0
        return conn.execute("SELECT count(*) FROM audit").fetchone()[0]
    finally:conn.close()

def basics(tmp):
    db=tmp/"basic.sqlite"
    meta=run("init",db)
    assert meta["schema_version"]==1 and meta["mode"]=="sqlite-wal"
    assert run("version",db)["experimental"]
    assert run("plan-put",db,"demo","key","значение")["expected_revision"]==0
    a=run("put",db,"demo","key","значение","--expect",0,
          "--request-key","request-basic-0001")
    assert a["revision"]==1 and not a["replayed"]
    b=run("put",db,"demo","key","значение","--expect",0,
          "--request-key","request-basic-0001")
    assert b["replayed"] and b["audit_id"]==a["audit_id"]
    assert run("get",db,"demo","key")["value"]=="значение"
    assert run("list",db,"demo")[0]["revision"]==1
    assert run("put",db,"demo","key","different","--expect",0,
               "--request-key","request-basic-0001",ok=False).returncode!=0
    assert run("put",db,"demo","key","changed","--expect",0,
               "--request-key","request-basic-0002",ok=False).returncode!=0
    assert run("plan-delete",db,"demo","key")["needs_confirmation"]
    run("delete",db,"demo","key","--expect",1,
        "--request-key","request-basic-0003",ok=False)
    r=run("delete",db,"demo","key","--expect",1,
          "--request-key","request-basic-0003","--confirm")
    assert r["revision"]==2
    assert run("get",db,"demo","key")["value"] is None
    assert run("list",db,"demo")==[]
    assert len(run("audit",db))==2
    assert run("verify",db)["integrity"]=="ok"
    assert check_sql(db)==2
    return db

def concurrency(tmp):
    db=tmp/"concurrency.sqlite"
    run("init",db)
    run("put",db,"demo","item","seed","--expect",0,"--request-key","request-seed-0001")
    planned=run("plan-put",db,"demo","item","next")
    assert planned["expected_revision"]==1
    def write(i):
        return run("put",db,"demo","item",f"changed{i}","--expect",1,
                   "--request-key",f"request-parallel-{i:04}",ok=(i==999))
    # Run multiple *real processes* through SQLite WAL's BEGIN IMMEDIATE.
    def trywrite(i):
        result=subprocess.run([str(BIN),"put",str(db),"demo","item",f"changed{i}",
             "--expect","1","--request-key",f"request-parallel-{i:04}"],
             capture_output=True,text=True,timeout=12,env={"PATH":"/nonexistent"})
        return result.returncode
    with concurrent.futures.ThreadPoolExecutor(max_workers=12) as pool:
        statuses=list(pool.map(trywrite,range(12)))
    assert statuses.count(0)==1,statuses
    assert statuses.count(2)==11,statuses
    assert run("get",db,"demo","item")["revision"]==2
    assert check_sql(db)==2

def crash_windows(tmp):
    stage_cases=[
        ("after_value_write",False),
        ("after_audit_write",False),
        ("before_commit",False),
        ("after_commit",True),
    ]
    for stage,committed in stage_cases:
        db=tmp/f"crash-{stage}.sqlite"
        run("init",db)
        failure=run("put",db,"org","key","safe","--expect",0,
                    "--request-key","request-crash-0001",
                    env={"SEDRAVASQL_TEST_FAULT":stage},exitcode=86)
        assert f"SEDRAVASQL_TEST_FAULT:{stage}" in failure.stderr
        check_sql(db)
        state=run("get",db,"org","key")
        if committed:
            assert state["value"]=="safe" and check_sql(db)==1
            repeat=run("put",db,"org","key","safe","--expect",0,
                       "--request-key","request-crash-0001")
            assert repeat["replayed"]
            assert check_sql(db)==1
        else:
            assert state is None and check_sql(db)==0
            result=run("put",db,"org","key","safe","--expect",0,
                       "--request-key","request-crash-0001")
            assert result["revision"]==1 and not result["replayed"]
            assert check_sql(db)==1
    return len(stage_cases)

def backup_wal(tmp):
    db=tmp/"backup-live.sqlite"
    copy=tmp/"backup-copy.sqlite"
    run("init",db)
    holder=sqlite3.connect(db)
    try:
        holder.execute("PRAGMA journal_mode=WAL")
        holder.execute("PRAGMA wal_autocheckpoint=0")
        holder.execute("INSERT INTO records(namespace,key,value,revision) VALUES('raw','uncheckpointed','YES',1)")
        holder.commit()
        assert Path(str(db)+"-wal").is_file()
        assert run("backup",db,copy)["backup"]=="created"
        assert run("get",copy,"raw","uncheckpointed")["value"]=="YES"
        assert run("verify",copy)["integrity"]=="ok"
        run("backup",db,copy,ok=False)
    finally:holder.close()
    assert check_sql(copy)==0

def malicious(tmp):
    db=tmp/"reject.sqlite"
    run("init",db)
    run("put",db,"bad namespace","item","x","--expect",0,
        "--request-key","request-unsafe-0001",ok=False)
    run("put",db,"demo","item", "x"*65537,"--expect",0,
        "--request-key","request-unsafe-0002",ok=False)
    run("delete",db,"demo","missing","--expect",0,
        "--request-key","request-unsafe-0003","--confirm",ok=False)
    assert check_sql(db)==0
    alias=tmp/"alias.sqlite"
    alias.symlink_to(db)
    run("init",alias,ok=False)
    run("get",alias,"demo","item",ok=False)
    foreign=tmp/"foreign.sqlite"
    conn=sqlite3.connect(foreign)
    conn.executescript("CREATE TABLE existing_data(secret TEXT);INSERT INTO existing_data VALUES('keep');")
    conn.close()
    run("init",foreign,ok=False)
    conn=sqlite3.connect(foreign)
    assert conn.execute("SELECT secret FROM existing_data").fetchone()[0]=="keep"
    conn.close()

def main():
    assert BIN.is_file(),BIN
    with tempfile.TemporaryDirectory(prefix="sedravasql-v01-") as td:
        root=Path(td)
        basics(root)
        concurrency(root)
        fault_count=crash_windows(root)
        backup_wal(root)
        malicious(root)
    print("PASS native CLI init/version/plan/put/get/list/delete/audit/verify")
    print("PASS idempotent replay and conflicting key reject; stale CAS and delete confirmation reject")
    print("PASS 12 separate concurrent processes: 1 successful writer and 11 stale-revision conflicts")
    print(f"PASS {fault_count} debug crash windows: SQLite state + audit + receipts atomic on restart")
    print("PASS online backup preserves committed WAL data with separate SQLite writer still open")
    print("PASS foreign schema and symlinks refused, oversized/missing/deleted values reject")
    print("PASS all subprocesses PATH=/nonexistent, no server ports or production data")

if __name__=="__main__":
    main()
