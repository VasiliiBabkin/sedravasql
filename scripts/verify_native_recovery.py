#!/usr/bin/env python3
"""Operator-gated source-preserving native log recovery smoke test.

The scripted removal of a stale lock is permitted only after the disposable
child process has exited with the debug-only controlled fault code. It must
never be used against a real service's database.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]
EXE=Path(os.environ.get("SEDRAVASQL_NATIVE_BIN",str(ROOT/"native-engine/target/debug/sedravasql-native-preview")))
def cmd(*args,expected=0,fault=None):
    env={"PATH":"/nonexistent","LC_ALL":"C"}
    if fault:
        env["SEDRAVASQL_NATIVE_TEST_FAULT"]=fault
    p=subprocess.run([str(EXE),*map(str,args)],env=env,
                     capture_output=True,text=True,timeout=20)
    assert p.returncode==expected,(p.returncode,expected,p.stdout,p.stderr)
    return p
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
def put(value,rev):
    return [{"namespace":"demo","key":"balance","expected_revision":rev,
             "change":{"kind":"Put","value":value}}]

def main():
    assert EXE.is_file(),EXE
    with tempfile.TemporaryDirectory(prefix="sedravasql-native-recovery-") as temp:
        root=Path(temp);os.chmod(root,0o700)
        source=root/"original.slog"
        restored=root/"restored.slog"
        ops=root/"ops.json"
        ops.write_text(json.dumps(put("200",0)))
        cmd("init",source)
        cmd("batch",source,"recovery-initial-00001",ops)
        initial_len=source.stat().st_size
        ops.write_text(json.dumps(put("300",1)))
        crash=cmd("batch",source,"recovery-incomplete-0001",ops,fault="after_length",expected=86)
        assert "after_length" in crash.stderr
        lock=source.with_name("original.slog.lock")
        assert lock.exists()
        cmd("recovery-plan",source,expected=2)
        cmd("recovery-copy",source,restored,"a"*64,expected=2)
        assert not restored.exists()
        # The child is confirmed stopped. This lock is created by this test.
        lock.unlink()
        plan=json.loads(cmd("recovery-plan",source).stdout)
        oldhash=digest(source)
        assert plan["source_sha256"]==oldhash
        assert plan["verified_prefix_bytes"]==initial_len
        assert plan["incomplete_tail_bytes"]>0
        assert plan["recovered_sequence"]==1
        cmd("recovery-copy",source,restored,"f"*64,expected=2)
        assert not restored.exists()
        copied=json.loads(cmd("recovery-copy",source,restored,oldhash).stdout)
        assert copied==plan
        assert digest(source)==oldhash
        assert restored.stat().st_size==initial_len
        assert json.loads(cmd("verify",restored).stdout)["sequence"]==1
        assert json.loads(cmd("read",restored,"demo","balance").stdout)["value"]=="200"
        ops.write_text(json.dumps(put("200",0)))
        assert json.loads(cmd("batch",restored,"recovery-initial-00001",ops).stdout)["replayed"]
        cmd("recovery-copy",source,restored,oldhash,expected=2)
        print("PASS experimental CLI plan + operator-approved copy; original and existing destination untouched")

        # A fully present, incorrect digest must never be offered for recovery.
        damaged=root/"checksum.slog"
        data=bytearray(restored.read_bytes());data[-1]^=1;damaged.write_bytes(data)
        cmd("recovery-plan",damaged,expected=2)
        cmd("recovery-copy",damaged,root/"denied.slog",digest(damaged),expected=2)
        assert not (root/"denied.slog").exists()
        print("PASS complete bad checksum rejected, no silent truncation")
        # A complete synced commit followed by a lost response is NOT a torn tail.
        late=root/"synced.slog"
        cmd("init",late)
        ops.write_text(json.dumps(put("400",0)))
        crash=cmd("batch",late,"recovery-synced-00001",ops,fault="after_sync",expected=86)
        assert "after_sync" in crash.stderr
        late.with_name("synced.slog.lock").unlink()
        cmd("recovery-plan",late,expected=2)
        assert json.loads(cmd("verify",late).stdout)["sequence"]==1
        assert json.loads(cmd("batch",late,"recovery-synced-00001",ops).stdout)["replayed"]
        print("PASS fully synced lost reply preserves committed outcome and original receipt")
    print("PASS isolated temporary recovery tests cleaned; no production files or public ports")

if __name__=="__main__":
    main()
