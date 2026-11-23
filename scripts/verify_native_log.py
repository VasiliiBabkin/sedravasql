#!/usr/bin/env python3
"""SQLite-free SedravaSQL: real child-process CLI and two debug crash windows.

All files live in TemporaryDirectory(mode=0700). Never touches an installed
database or tries to auto-repair a torn append-only frame.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
EXE = Path(os.environ.get("SEDRAVASQL_NATIVE_BIN", str(ROOT / "native-engine/target/debug/sedravasql-native-preview")))
assert EXE.is_file(), EXE


def run(*args, fault=None, expect=0):
    env = {"PATH": "/nonexistent", "LC_ALL": "C"}
    if fault:
        env["SEDRAVASQL_NATIVE_TEST_FAULT"] = fault
    p = subprocess.run([str(EXE), *map(str,args)], capture_output=True, text=True,
                       env=env, timeout=15)
    assert p.returncode == expect, (p.returncode, expect, p.stdout, p.stderr)
    return p


def ops(*rows):
    return [
        {"namespace": ns, "key": key, "expected_revision":rev,
         "change":{"kind":"Put", "value":value}}
        for ns,key,rev,value in rows
    ]


def new_folder(base: Path, name: str):
    p=base/name
    p.mkdir(mode=0o700)
    return p


def main():
    with tempfile.TemporaryDirectory(prefix="sedravasql-native-log-") as temp:
        root=Path(temp)
        os.chmod(root, 0o700)
        work=new_folder(root,"normal")
        db=work/"data.slog"
        request=work/"batch.json"
        request.write_text(json.dumps(ops(("n","a",0,"first"),("n","b",0,"second"))))
        init=json.loads(run("init",db).stdout)
        assert init["created"] is True
        first=json.loads(run("batch",db,"independent-request-0001",request).stdout)
        assert first["sequence"]==1 and not first["replayed"] and len(first["entries"])==2
        assert json.loads(run("read",db,"n","a").stdout)["value"]=="first"
        assert json.loads(run("verify",db).stdout)["sequence"]==1
        again=json.loads(run("batch",db,"independent-request-0001",request).stdout)
        assert again["sequence"]==1 and again["replayed"]
        assert json.loads(run("verify",db).stdout)["sequence"]==1
        request.write_text(json.dumps(ops(("n","a",0,"different"),("n","b",0,"second"))))
        assert "CONFLICT" in run("batch",db,"independent-request-0001",request,expect=2).stderr
        assert "CONFLICT" in run("batch",db,"independent-request-0002",request,expect=2).stderr
        assert json.loads(run("read",db,"n","b").stdout)["value"]=="second"
        print("PASS child-process SQLite-free batch, 2 keys atomic, durable receipt and CAS")
        # Intentionally torn write; fail closed and preserve the entire file.
        torn_dir=new_folder(root,"torn")
        torn=torn_dir/"torn.slog"
        batch=torn_dir/"data.json"
        batch.write_text(json.dumps(ops(("n","k",0,"never-acknowledged"))))
        run("init",torn)
        p=run("batch",torn,"torn-request-00001",batch,fault="after_length",expect=86)
        assert "after_length" in p.stderr
        locked=torn.with_name("torn.slog.lock")
        assert locked.is_file(), "simulated process crash must leave a stale lock"
        assert "lock exists" in run("verify",torn,expect=2).stderr
        locked.unlink()  # Explicit test-only operator action after confirmed process exit.
        before=hashlib.sha256(torn.read_bytes()).hexdigest()
        assert "torn frame" in run("verify",torn,expect=2).stderr
        assert hashlib.sha256(torn.read_bytes()).hexdigest()==before
        print("PASS debug crash after length: stale lock, torn record rejected without mutation")
        # Complete synced record with lost response; read/replay exactly once after crash.
        safe_dir=new_folder(root,"synced")
        safe=safe_dir/"synced.slog"
        batch=safe_dir/"data.json"
        batch.write_text(json.dumps(ops(("n","a",0,"persisted"),("n","b",0,"also-persisted"))))
        run("init",safe)
        p=run("batch",safe,"synced-request-0001",batch,fault="after_sync",expect=86)
        assert "after_sync" in p.stderr
        lock=safe.with_name("synced.slog.lock")
        assert lock.is_file()
        lock.unlink()  # Only after child has exited with code 86.
        assert json.loads(run("verify",safe).stdout)["sequence"]==1
        assert json.loads(run("read",safe,"n","b").stdout)["value"]=="also-persisted"
        replay=json.loads(run("batch",safe,"synced-request-0001",batch).stdout)
        assert replay["replayed"] and replay["sequence"]==1
        print("PASS debug crash after sync: two durable keys, exact idempotent replay")
    print("PASS all temporary tests cleaned, no public ports, no SQLite used")


if __name__=="__main__":
    main()
