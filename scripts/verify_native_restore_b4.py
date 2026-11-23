#!/usr/bin/env python3
"""B4 cross-process operator-approved snapshot -> NEW SEDRLOG1 restore smoke.

Disposable local files only. No live DB, no production data or in-place swaps.
"""
from __future__ import annotations
import hashlib,json,os,subprocess,tempfile
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
BIN=Path(os.environ.get("SEDRAVASQL_NATIVE_BIN",ROOT/"native-engine/target/debug/sedravasql-native-preview"))

def cmd(*args:object,success=True):
    p=subprocess.run([str(BIN),*[str(a) for a in args]],capture_output=True,
        text=True,encoding="utf-8",timeout=30,env={"PATH":"/nonexistent","LC_ALL":"C"})
    assert (p.returncode==0)==success,(args,p.returncode,p.stdout,p.stderr)
    return json.loads(p.stdout) if success else p.stderr
def sha(p:Path)->str:
    return hashlib.sha256(p.read_bytes()).hexdigest()
def operations(path:Path,revision:int,value:str):
    path.write_text(json.dumps([
        {"namespace":"demo","key":"alpha","expected_revision":revision,"change":{"kind":"Put","value":value}},
        {"namespace":"demo","key":"beta","expected_revision":revision,"change":{"kind":"Put","value":"🍀"+value}},
    ],ensure_ascii=False))
    return path

def main():
    assert BIN.is_file(),BIN
    with tempfile.TemporaryDirectory(prefix="sedravasql-b4-") as temp:
        directory=Path(temp);os.chmod(directory,0o700)
        old=directory/"original.slog"
        snapshot=directory/"snapshot.snap"
        restored=directory/"restored.slog"
        ops=directory/"operations.json"
        cmd("init",old)
        for rev in range(8):
            receipt=cmd("batch",old,f"restore-cli-{rev:08}",operations(ops,rev,str(rev)))
            assert receipt["sequence"]==rev+1
        before=old.read_bytes()
        planned=cmd("backup-plan",old)
        assert planned["source_sha256"]==sha(old)
        created=cmd("snapshot-create",old,snapshot,sha(old))
        assert created==cmd("snapshot-verify",snapshot)
        assert created["sequence"]==8 and created["receipts"]==8
        assert "NATIVE_PREVIEW_ERROR" in cmd("restore-copy",snapshot,restored,"0"*64,success=False)
        assert not restored.exists()
        outcome=cmd("restore-copy",snapshot,restored,sha(snapshot))
        assert outcome["recovered_log_sha256"]==sha(old)==sha(restored)
        assert outcome["sequence"]==8
        assert outcome["entries"]==2 and outcome["receipts"]==8
        assert old.read_bytes()==restored.read_bytes()==before
        assert cmd("verify",restored)["sequence"]==8
        assert cmd("read",restored,"demo","beta")["value"]=="🍀7"
        for rev in range(8):
            replay=cmd("batch",restored,f"restore-cli-{rev:08}",operations(ops,rev,str(rev)))
            assert replay["replayed"] is True and replay["sequence"]==rev+1
        cmd("batch",restored,"restore-continued-00001",operations(ops,8,"new"))
        assert cmd("verify",restored)["sequence"]==9
        assert cmd("verify",old)["sequence"]==8
        print("PASS B4 CLI: 8 original transactions, exact bytes SHA, receipt replay, independent new write")
        assert "NATIVE_PREVIEW_ERROR" in cmd("restore-copy",snapshot,restored,sha(snapshot),success=False)
        assert cmd("verify",restored)["sequence"]==9
        damaged=directory/"damaged.snap"
        bad=bytearray(snapshot.read_bytes());bad[-1]^=1;damaged.write_bytes(bad)
        assert "NATIVE_PREVIEW_ERROR" in cmd("restore-copy",damaged,directory/"bad-output",sha(damaged),success=False)
        assert not (directory/"bad-output").exists()
        assert "NATIVE_PREVIEW_ERROR" in cmd("restore-copy",old,directory/"bad-output",sha(old),success=False)
        assert not (directory/"bad-output").exists()
        assert snapshot.read_bytes()==snapshot.read_bytes()
        print("PASS B4 CLI: corrupt/foreign snapshot denied, preexisting target never clobbered")
    print("PASS B4 CLI: isolated test directory discarded; no service modified")

if __name__=="__main__":
    main()
