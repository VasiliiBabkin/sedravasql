#!/usr/bin/env python3
"""B3 separate-process offline materialized snapshots on disposable native logs.

No production data, no restore/compaction, no live writer.
"""
from __future__ import annotations
import hashlib,json,os,subprocess,tempfile
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
BIN=Path(os.environ.get("SEDRAVASQL_NATIVE_BIN",ROOT/"native-engine/target/debug/sedravasql-native-preview"))

def call(*args:object, success=True):
    p=subprocess.run([str(BIN),*[str(x) for x in args]],
        capture_output=True,text=True,encoding="utf-8",timeout=30,
        env={"PATH":"/nonexistent","LC_ALL":"C"})
    assert (p.returncode==0)==success,(args,p.returncode,p.stdout,p.stderr)
    return json.loads(p.stdout) if success else p.stderr
def hashfile(p:Path)->str:
    return hashlib.sha256(p.read_bytes()).hexdigest()

def main():
    assert BIN.is_file(),BIN
    with tempfile.TemporaryDirectory(prefix="sedravasql-b3-snapshot-") as t:
        root=Path(t);os.chmod(root,0o700)
        src=root/"data.slog"
        first=root/"first.snapshot"
        second=root/"second.snapshot"
        ops=root/"operations.json"
        call("init",src)
        for rev in range(8):
            ops.write_text(json.dumps([
                {"namespace":"demo","key":"alpha","expected_revision":rev,"change":{"kind":"Put","value":f"data-{rev}"}},
                {"namespace":"demo","key":"beta","expected_revision":rev,"change":{"kind":"Put","value":f"other-{rev}"}}
            ]))
            commit=call("batch",src,f"snapshot-cli-{rev:08}",ops)
            assert commit["sequence"]==rev+1
        original=src.read_bytes()
        plan=call("backup-plan",src)
        assert plan["source_sha256"]==hashfile(src) and plan["sequence"]==8

        assert "NATIVE_PREVIEW_ERROR" in call("snapshot-create",src,first,"0"*64,success=False)
        assert not first.exists()
        made=call("snapshot-create",src,first,plan["source_sha256"])
        checked=call("snapshot-verify",first)
        assert made==checked
        assert made["source_sha256"]==plan["source_sha256"]
        assert made["sequence"]==8 and made["entries"]==2 and made["receipts"]==8
        assert made["snapshot_sha256"]==hashfile(first)
        assert made["snapshot_bytes"]==first.stat().st_size
        twice=call("snapshot-create",src,second,plan["source_sha256"])
        assert made==twice
        assert first.read_bytes()==second.read_bytes()
        assert src.read_bytes()==original
        assert "NATIVE_PREVIEW_ERROR" in call("verify",first,success=False)
        print("PASS B3: 8 commits, 2 keys, all receipts, byte-identical deterministic materialization")
        print("PASS B3: checksum/source hash and independent snapshot verification")
        assert "NATIVE_PREVIEW_ERROR" in call("snapshot-create",src,first,plan["source_sha256"],success=False)
        assert first.read_bytes()==second.read_bytes()
        damaged=root/"damaged.snapshot"
        bad=bytearray(first.read_bytes());bad[-1]^=1;damaged.write_bytes(bad)
        assert "NATIVE_PREVIEW_ERROR" in call("snapshot-verify",damaged,success=False)
        assert "NATIVE_PREVIEW_ERROR" in call("snapshot-verify",src,success=False)
        print("PASS B3: refusing corrupted snapshot and source-as-snapshot")

        lock=root/"data.slog.lock"
        lock.write_text("writer?")
        assert "NATIVE_PREVIEW_ERROR" in call("snapshot-create",src,root/"busy.snapshot",plan["source_sha256"],success=False)
        assert not (root/"busy.snapshot").exists()
        lock.unlink()
        ops.write_text(json.dumps([{
            "namespace":"demo","key":"alpha","expected_revision":8,
            "change":{"kind":"Put","value":"new"}
        }]))
        call("batch",src,"snapshot-after-plan01",ops)
        assert "NATIVE_PREVIEW_ERROR" in call("snapshot-create",src,root/"stale.snapshot",plan["source_sha256"],success=False)
        assert not (root/"stale.snapshot").exists()
        print("PASS B3: lock rejection, stale plan rejection, new files never overwritten")
    print("PASS B3: temporary test data removed, no production data accessed")


if __name__=="__main__":
    main()
