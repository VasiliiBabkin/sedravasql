#!/usr/bin/env python3
"""B2 operator-approved offline native log backup in separate CLI processes."""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]
EXE=Path(os.environ.get("SEDRAVASQL_NATIVE_BIN",ROOT/"native-engine/target/debug/sedravasql-native-preview"))

def run(*args:object,success:bool=True):
    p=subprocess.run(
        [str(EXE),*[str(x) for x in args]],
        text=True,capture_output=True,timeout=30,
        env={"PATH":"/nonexistent","LC_ALL":"C"},
    )
    assert (p.returncode==0)==success,(p.returncode,args,p.stdout,p.stderr)
    return json.loads(p.stdout) if p.returncode==0 else p.stderr
def digest(path:Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()
def ops(directory:Path,value:str,revision:int):
    name=directory/"ops.json"
    name.write_text(json.dumps([
        {"namespace":"demo","key":"left","expected_revision":revision,"change":{"kind":"Put","value":value}},
        {"namespace":"demo","key":"right","expected_revision":revision,"change":{"kind":"Put","value":"x"+value}},
    ]))
    return name
def main():
    assert EXE.is_file(),EXE
    with tempfile.TemporaryDirectory(prefix="sedravasql-backup-b2-") as temp:
        folder=Path(temp);os.chmod(folder,0o700)
        source=folder/"src.slog";dest=folder/"clean-backup.slog"
        run("init",source)
        for rev in range(8):
            run("batch",source,f"backup-cli-0000{rev:04}",ops(folder,str(rev),rev))
        expected_source=digest(source)
        plan=run("backup-plan",source)
        assert plan=={"source_sha256":expected_source,"source_bytes":source.stat().st_size,"sequence":8}
        assert "NATIVE_PREVIEW_ERROR" in run("backup-copy",source,dest,"0"*64,success=False)
        assert not dest.exists()
        copy=run("backup-copy",source,dest,expected_source)
        assert copy==plan
        assert dest.read_bytes()==source.read_bytes()
        assert digest(dest)==expected_source
        assert run("verify",dest)["sequence"]==8
        assert run("read",dest,"demo","right")["value"]=="x7"
        replay=run("batch",dest,"backup-cli-00000007",ops(folder,"7",7))
        assert replay["replayed"] is True
        assert "NATIVE_PREVIEW_ERROR" in run("backup-copy",source,dest,expected_source,success=False)
        assert digest(source)==digest(dest)
        print("PASS B2: 8 transactions, two keys, byte-identical copy, checksum and receipts")
        print("PASS B2: explicit hash authorization, output never overwritten")

        busy=folder/"busy.slog"
        busy.write_bytes(source.read_bytes())
        busy.with_name("busy.slog.lock").write_text("writer alive?")
        assert "NATIVE_PREVIEW_ERROR" in run("backup-plan",busy,success=False)
        assert "NATIVE_PREVIEW_ERROR" in run("backup-copy",busy,folder/"blocked.slog",digest(busy),success=False)
        assert not (folder/"blocked.slog").exists()
        print("PASS B2: busy lock denied, no stale-lock auto removal")

        torn=folder/"torn.slog"
        torn.write_bytes(source.read_bytes()+b"\x00")
        assert "NATIVE_PREVIEW_ERROR" in run("backup-plan",torn,success=False)
        assert "NATIVE_PREVIEW_ERROR" in run("backup-copy",torn,folder/"blocked.slog",digest(torn),success=False)
        print("PASS B2: torn journal denied, no partial backup")

        damaged=folder/"damaged.slog"
        data=bytearray(source.read_bytes());data[-1]^=1;damaged.write_bytes(data)
        assert "NATIVE_PREVIEW_ERROR" in run("backup-plan",damaged,success=False)
        assert "NATIVE_PREVIEW_ERROR" in run("backup-copy",damaged,folder/"blocked.slog",digest(damaged),success=False)
        print("PASS B2: complete corrupt checksum denied")

        newer=run("batch",source,"backup-cli-after-plan",ops(folder,"8",8))
        assert newer["sequence"]==9
        assert "NATIVE_PREVIEW_ERROR" in run("backup-copy",source,folder/"stale.slog",expected_source,success=False)
        assert not (folder/"stale.slog").exists()
        print("PASS B2: mutation after approved plan invalidates operator hash")
    print("PASS B2: isolated ephemeral CLI files removed")


if __name__=="__main__":
    main()
