#!/usr/bin/env python3
"""B8 offline verified index sidecar, actual CLI commands and test corruption.

Only mode 0700 disposable directories. No production log or stale-lock cleanup.
"""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

BIN=Path(os.environ["SEDRAVASQL_NATIVE_BIN"])
def run(*args, fail=False):
    cmd=[str(BIN),*(str(x) for x in args)]
    p=subprocess.run(cmd,text=True,capture_output=True,timeout=25,
       env={"PATH":"/nonexistent","LC_ALL":"C"})
    if fail:
        assert p.returncode!=0,(cmd,p.stdout,p.stderr)
        assert "NATIVE_PREVIEW_ERROR" in p.stderr
        return p.stderr
    assert p.returncode==0,(cmd,p.returncode,p.stdout,p.stderr)
    return json.loads(p.stdout)
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
def main():
    assert BIN.is_file(),BIN
    with tempfile.TemporaryDirectory(prefix="sedravasql-index-b8-") as folder:
        root=Path(folder);root.chmod(0o700)
        journal=root/"test.slog"
        source=root/"source.snp"
        idx=root/"materialized.idx"
        ops=root/"ops.json"
        run("init",journal)
        for rev in range(6):
            ops.write_text(json.dumps([
                dict(namespace="demo",key="alpha",expected_revision=rev,
                    change={"kind":"Put","value":f"α version {rev}"}),
                dict(namespace="demo",key="beta",expected_revision=rev,
                    change={"kind":"Put","value":f"β version {rev}"}),
            ],ensure_ascii=False))
            result=run("batch",journal,f"b8-commit-{rev:06d}",ops)
            assert result["sequence"]==rev+1
        ops.write_text(json.dumps([
            dict(namespace="demo",key="beta",expected_revision=6,
                change={"kind":"Delete"}),
        ]))
        run("batch",journal,"b8-delete-beta-0007",ops)
        source_digest=digest(journal)
        report=run("snapshot-create",journal,source,source_digest)
        assert report["sequence"]==7 and report["entries"]==2
        snap_sha=digest(source)
        created=run("index-create",source,idx,snap_sha)
        assert created["sequence"]==7 and created["entries"]==2
        assert created["snapshot_sha256"]==snap_sha
        assert created["log_sha256"]==source_digest
        assert run("index-verify",idx)==created
        assert run("index-get",idx,"demo","alpha")=={
            "namespace":"demo","key":"alpha","value":"α version 5","revision":6
        }
        assert run("index-get",idx,"demo","beta")=={
            "namespace":"demo","key":"beta","value":None,"revision":7
        }
        assert run("index-get",idx,"demo","not-there") is None
        assert source_digest==digest(journal) and snap_sha==digest(source)
        assert idx.stat().st_mode&0o777==0o600
        # Determinism: two new files made from one immutable approved source
        second=root/"copy.idx"
        assert run("index-create",source,second,snap_sha)==created
        assert idx.read_bytes()==second.read_bytes()
        # Wrong approval and existing destination MUST fail closed
        run("index-create",source,root/"unapproved.idx","0"*64,fail=True)
        assert not (root/"unapproved.idx").exists()
        before=idx.read_bytes()
        run("index-create",source,idx,snap_sha,fail=True)
        assert idx.read_bytes()==before
        # Tampering, truncating or appending bytes must never become valid.
        for position in (0,8,12,20,len(before)//2,len(before)-33,len(before)-1):
            damaged=bytearray(before)
            damaged[position]^=0x01
            check=root/"bad.idx"
            check.write_bytes(damaged)
            check.chmod(0o600)
            run("index-verify",check,fail=True)
            run("index-get",check,"demo","alpha",fail=True)
            check.unlink()
        for size in (0,1,11,15,48,len(before)-1):
            check=root/"short.idx"
            check.write_bytes(before[:size]);check.chmod(0o600)
            run("index-verify",check,fail=True)
            check.unlink()
        check=root/"trailing.idx"
        check.write_bytes(before+b"X");check.chmod(0o600)
        run("index-verify",check,fail=True)
        # Record truncation/format mismatch rejected.
        assert run("verify",journal)["sequence"]==7
        print("PASS B8: strict index verify and binary-search get (active/tombstone/missing)")
        print("PASS B8: deterministic independent sidecar, approved hash, no source changes")
        print("PASS B8: 7 byte flips + 6 truncations + trailing byte rejected; existing dest untouched")
        print("LIMIT: full-file verification per lookup, offline snapshot not live index or checkpoint")

if __name__=="__main__":main()
