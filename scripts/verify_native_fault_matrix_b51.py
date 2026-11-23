#!/usr/bin/env python3
"""B5.1 deterministic offline corruption/fault-input matrix for B3/B4.

Tests ordinary disk images with truncation, mutation, append and preexisting
partial output. NOT true crash injection or proof of fsync/power-loss durability.
"""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import random
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]
BIN=Path(os.environ.get("SEDRAVASQL_NATIVE_BIN",ROOT/"native-engine/target/debug/sedravasql-native-preview"))

def cmd(*args:object, ok:bool=True):
    p=subprocess.run([str(BIN),*(str(s) for s in args)],
        capture_output=True,text=True,encoding="utf-8",timeout=30,
        env={"PATH":"/nonexistent","LC_ALL":"C"})
    if ok:
        assert p.returncode==0,(args,p.returncode,p.stdout,p.stderr)
        return json.loads(p.stdout)
    assert p.returncode!=0,(args,p.stdout,p.stderr)
    assert "NATIVE_PREVIEW_ERROR" in p.stderr,(args,p.stderr)
    return None

def digest(data:bytes)->str:
    return hashlib.sha256(data).hexdigest()

def negative_snapshot(candidate:Path,destination:Path,payload:bytes):
    candidate.write_bytes(payload)
    if destination.exists():destination.unlink()
    cmd("snapshot-verify",candidate,ok=False)
    cmd("restore-copy",candidate,destination,digest(payload),ok=False)
    assert not destination.exists(),"Faulty snapshot unexpectedly produced output"


def main():
    assert BIN.is_file(),BIN
    rnd=random.Random(0xB5B5)
    altered=0
    truncated=0
    appended=0
    preserved_partial=0
    positive=0
    with tempfile.TemporaryDirectory(prefix="sedravasql-b5-matrix-") as dirname:
        root=Path(dirname)
        os.chmod(root,0o700)
        for seed in range(5):
            directory=root/f"case-{seed:02d}"
            directory.mkdir(mode=0o700)
            src=directory/"original.slog"
            snap=directory/"good.snap"
            restored=directory/"restored.slog"
            wrong=directory/"bad.snap"
            rejected=directory/"refused.slog"
            operations=directory/"ops.json"
            cmd("init",src)
            count=5+seed*2
            for rev in range(count):
                ops=[
                    {"namespace":"matrix","key":"alpha","expected_revision":rev,
                     "change":{"kind":"Put","value":f"value-{seed}-{rev}-🐱"}},
                    {"namespace":"matrix","key":"beta","expected_revision":rev,
                     "change":{"kind":"Put","value":f"🍀-{seed}-{rev}"}}
                ]
                operations.write_text(json.dumps(ops,ensure_ascii=False))
                receipt=cmd("batch",src,f"b5-case-{seed:02}-step-{rev:06}",operations)
                assert receipt["sequence"]==rev+1
            original=src.read_bytes()
            plan=cmd("backup-plan",src)
            assert plan["source_sha256"]==digest(original)
            made=cmd("snapshot-create",src,snap,plan["source_sha256"])
            clean=snap.read_bytes()
            assert made["snapshot_sha256"]==digest(clean)
            assert cmd("snapshot-verify",snap)==made
            recovered=cmd("restore-copy",snap,restored,digest(clean))
            assert recovered["recovered_log_sha256"]==digest(original)
            assert restored.read_bytes()==original
            assert cmd("verify",restored)["sequence"]==count
            assert cmd("read",restored,"matrix","beta")["value"]==f"🍀-{seed}-{count-1}"
            # Identical transaction keys in reconstructed history must be replayed.
            for rev in (0,count//2,count-1):
                operations.write_text(json.dumps([
                    {"namespace":"matrix","key":"alpha","expected_revision":rev,
                     "change":{"kind":"Put","value":f"value-{seed}-{rev}-🐱"}},
                    {"namespace":"matrix","key":"beta","expected_revision":rev,
                     "change":{"kind":"Put","value":f"🍀-{seed}-{rev}"}}
                ],ensure_ascii=False))
                result=cmd("batch",restored,f"b5-case-{seed:02}-step-{rev:06}",operations)
                assert result["replayed"] and result["sequence"]==rev+1
            assert src.read_bytes()==original
            positive+=1

            # Spot sample byte corruption covering magic, frame size, body,
            # trailing digest plus 20 seeded positions (no checksum repair).
            locations={0,1,7,8,11,12,len(clean)//2,len(clean)-33,len(clean)-1}
            locations.update(rnd.sample(range(len(clean)),min(20,len(clean))))
            for pos in sorted(locations):
                corrupted=bytearray(clean)
                corrupted[pos]^=0x01
                negative_snapshot(wrong,rejected,bytes(corrupted))
                altered+=1

            # These are synthetic truncation tests, not interrupted fsync.
            cutoffs={0,1,7,8,11,12,13,len(clean)//3,len(clean)//2,
                     len(clean)-33,len(clean)-32,len(clean)-1}
            for n in sorted(x for x in cutoffs if 0<=x<len(clean)):
                negative_snapshot(wrong,rejected,clean[:n])
                truncated+=1

            # Trailing bytes are rejected even if the original checksum remains.
            for extra in (b"\x00",b"X"*17,b"garbage"+b"\x00"*64):
                negative_snapshot(wrong,rejected,clean+extra)
                appended+=1

            # Imitate a non-atomic previous job that left a truncated file.
            for n in (1,8,len(original)//2,len(original)-1):
                partial=directory/f"partial-{n}.slog"
                partial.write_bytes(original[:n])
                expected=partial.read_bytes()
                cmd("restore-copy",snap,partial,digest(clean),ok=False)
                assert partial.read_bytes()==expected
                preserved_partial+=1
        print(f"PASS B5.1: {positive} deterministic snapshot -> exact journal -> replay roundtrips")
        print(f"PASS B5.1: {altered} flipped-byte, {truncated} truncated, {appended} extra-tail inputs rejected")
        print(f"PASS B5.1: {preserved_partial} pre-existing partial destinations never overwritten")
        print("LIMIT: simulates damaged files; no SIGKILL, power loss, kernel fsync or online writer tests")

if __name__=="__main__":main()
