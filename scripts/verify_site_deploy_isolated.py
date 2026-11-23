#!/usr/bin/env python3
"""Exercise actual deploy algorithm in a disposable fake document root.

Privilege/ownership operations are simulated, not live FastPanel writes.
This verifies the new docs/ directory path and preexisting-file backups.
"""
from __future__ import annotations
import hashlib
import importlib.util
import os
from pathlib import Path
import pwd
import sys
import tempfile
from types import SimpleNamespace

PATH=Path(__file__).resolve().with_name('deploy_official_site.py')
spec=importlib.util.spec_from_file_location("sedravasql_test_site_deploy",PATH)
assert spec is not None and spec.loader is not None
mod=importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
def sha(p:Path):
    return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
    with tempfile.TemporaryDirectory(prefix='sedravasql-website-deploy-test-') as t:
        root=Path(t)
        live=root/'site'
        live.mkdir(mode=0o755)
        (live/'index.html').write_text('old site, keep me in backup')
        backup=root/'backups'
        actual_uid=os.getuid()
        actual_gid=os.getgid()
        old=(mod.DEST,mod.BACKUPS,mod.os.geteuid,mod.os.chown,mod.pwd.getpwnam,list(sys.argv))
        try:
            mod.DEST=live
            mod.BACKUPS=backup
            mod.os.geteuid=lambda:0
            mod.os.chown=lambda path,uid,gid: None
            mod.pwd.getpwnam=lambda username:SimpleNamespace(pw_uid=actual_uid,pw_gid=actual_gid)
            sys.argv=['verify_site_deploy_isolated.py','--deploy']
            mod.run()
            for name in mod.FILES:
                assert (live/name).is_file(),name
                assert sha(live/name)==sha(mod.SRC/name),name
            dirs=list(backup.iterdir())
            assert len(dirs)==1
            assert (dirs[0]/'index.html').read_text()=='old site, keep me in backup'
            assert (live/'docs').is_dir()
            assert (live/'docs').stat().st_uid==actual_uid
            print("PASS: root-owned installer simulation creates new docs/ with proper owner")
            print("PASS: all files identical, previous site backed up outside public root")
            (live/'docs/index.html').unlink()
            (live/'docs/index.html').symlink_to(live/'index.html')
            try:
                mod.run()
            except ValueError:
                print("PASS: symbolic-link destination fails closed")
            else:
                raise AssertionError('Symlink destination unexpectedly allowed')
        finally:
            mod.DEST,mod.BACKUPS,mod.os.geteuid,mod.os.chown,mod.pwd.getpwnam,argv=old
            sys.argv=argv
if __name__=='__main__':main()
