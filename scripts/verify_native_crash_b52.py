#!/usr/bin/env python3
"""B5.2 process-abort crash windows for PRIVATE debug NativeLog journals.

This test only creates files inside its own TemporaryDirectory (0700).
The stale test lock is removed ONLY after verifying the child exited 86 and
its exact pid matches lock contents. NEVER reuse such cleanup on user data.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]
BIN=Path(os.environ.get("SEDRAVASQL_NATIVE_BIN",ROOT/"native-engine/target/debug/sedravasql-native-preview"))
ROUNDS=8

def cmd(*args, fail:bool=False):
    p=subprocess.run([str(BIN),*(str(x) for x in args)],capture_output=True,
        text=True,timeout=20,env={"PATH":"/nonexistent","LC_ALL":"C"})
    if fail:
        assert p.returncode!=0,(args,p.stdout,p.stderr)
        return p.stderr
    assert p.returncode==0,(args,p.returncode,p.stdout,p.stderr)
    return json.loads(p.stdout)

def sha(path:Path)->str:
    return hashlib.sha256(path.read_bytes()).hexdigest()

def ops(path:Path,rev:int,label:str):
    path.write_text(json.dumps([
        {"namespace":"crash","key":"alpha","expected_revision":rev,
         "change":{"kind":"Put","value":f"{label}-{rev}-🐱"}},
        {"namespace":"crash","key":"beta","expected_revision":rev,
         "change":{"kind":"Put","value":f"{label}-{rev}-🍀"}},
    ],ensure_ascii=False))
    return path

def crash(source:Path,key:str,operations:Path,point:str,isolated_root:Path):
    assert source.parent.is_relative_to(isolated_root)
    child=subprocess.Popen([str(BIN),"batch",str(source),key,str(operations)],
        stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,
        env={"PATH":"/nonexistent","LC_ALL":"C","SEDRAVASQL_NATIVE_TEST_FAULT":point})
    out,err=child.communicate(timeout=22)
    assert child.returncode==86,(point,child.returncode,out,err)
    assert f"SEDRAVASQL_NATIVE_TEST_FAULT:{point}" in err
    lock=source.with_name(source.name+".lock")
    info=lock.lstat()
    assert stat.S_ISREG(info.st_mode) and not lock.is_symlink()
    assert info.st_nlink==1 and info.st_uid==os.getuid()
    assert lock.read_text()==f"pid={child.pid}\n",(point,lock.read_text(),child.pid)
    # Both process and exact lock ownership/pid have been independently checked.
    assert child.poll() is not None
    assert "NATIVE_PREVIEW_ERROR" in cmd("verify",source,fail=True)
    assert "NATIVE_PREVIEW_ERROR" in cmd("recovery-plan",source,fail=True)
    lock.unlink()

def scenario(directory:Path,idx:int,point:str,root:Path):
    directory.mkdir(mode=0o700)
    source=directory/"source.slog"
    recovered=directory/"recovered.slog"
    input_path=directory/"ops.json"
    cmd("init",source)
    label=f"round{idx:02}"
    for rev in range(3):
        assert cmd("batch",source,f"{label}-initial-{rev:06}",ops(input_path,rev,label))["sequence"]==rev+1
    baseline=source.read_bytes()
    baseline_len=len(baseline)
    key=f"{label}-crash-request-{point}"
    operations=ops(input_path,3,label)
    crash(source,key,operations,point,root)
    current=source.read_bytes()
    assert current.startswith(baseline)
    if point in ("after_length","after_payload"):
        assert len(current)>baseline_len
        assert len(current)==baseline_len+4 if point=="after_length" else len(current)>baseline_len+4
        assert "NATIVE_PREVIEW_ERROR" in cmd("verify",source,fail=True)
        plan=cmd("recovery-plan",source)
        assert plan["recovered_sequence"]==3
        assert plan["verified_prefix_bytes"]==baseline_len
        assert plan["source_sha256"]==sha(source)
        assert plan["incomplete_tail_bytes"]==len(current)-baseline_len
        assert "NATIVE_PREVIEW_ERROR" in cmd("recovery-copy",source,recovered,"0"*64,fail=True)
        assert not recovered.exists()
        approval=sha(source)
        returned=cmd("recovery-copy",source,recovered,approval)
        assert returned==plan
        assert recovered.read_bytes()==baseline
        assert source.read_bytes()==current # source is never truncated
        assert cmd("verify",recovered)["sequence"]==3
        for rev in range(3):
            old=cmd("batch",recovered,f"{label}-initial-{rev:06}",ops(input_path,rev,label))
            assert old["replayed"] is True
        operations=ops(input_path,3,label)
        new=cmd("batch",recovered,key,operations)
        assert new["sequence"]==4 and not new["replayed"]
        assert cmd("verify",recovered)["sequence"]==4
        assert "NATIVE_PREVIEW_ERROR" in cmd("recovery-copy",source,recovered,approval,fail=True)
        assert source.read_bytes()==current
    else:
        assert point=="after_sync"
        assert len(current)>baseline_len
        assert cmd("verify",source)["sequence"]==4
        assert "NATIVE_PREVIEW_ERROR" in cmd("recovery-plan",source,fail=True)
        same=cmd("batch",source,key,operations)
        assert same["replayed"] is True and same["sequence"]==4
        altered=ops(input_path,3,label+"-changed")
        assert "NATIVE_PREVIEW_ERROR" in cmd("batch",source,key,altered,fail=True)
        after=cmd("batch",source,f"{label}-post-sync-00001",ops(input_path,4,label))
        assert after["sequence"]==5 and not after["replayed"]
        assert cmd("verify",source)["sequence"]==5

def main():
    assert BIN.is_file(),BIN
    with tempfile.TemporaryDirectory(prefix="sedravasql-b52-real-process-abort-") as tmp:
        root=Path(tmp)
        root.chmod(0o700)
        for point in ("after_length","after_payload","after_sync"):
            for idx in range(ROUNDS):
                scenario(root/f"{point}-{idx:02}",idx,point,root)
            print(f"PASS B5.2: {ROUNDS} actual debug process exits at {point} (exit 86)")
        print(f"PASS B5.2: {ROUNDS*3} crash windows, exact child PID/lock review")
        print(f"PASS B5.2: {ROUNDS*2} torn tails rejected + operator-reviewed copy into new journal")
        print(f"PASS B5.2: {ROUNDS} fully synced commits retained after lost reply; exact idempotent replay")
    print("LIMIT: deterministic debug exit after forced fsync, not sudden hardware power loss")

if __name__=="__main__":
    main()
