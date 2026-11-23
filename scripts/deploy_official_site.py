#!/usr/bin/env python3
"""Publish allowlisted static files to the existing FastPanel webroot.

Dry-run by default. No API token, sudo, webserver restart or other domains.
Every previous live file is backed up outside the webroot before publishing.
"""
from __future__ import annotations
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import time

ROOT=Path(__file__).resolve().parents[1]
SRC=ROOT/"site"
DEST=Path.home()/"www/sql.virtreal.ru"
BACKUPS=Path.home()/".sedrava-dev/site-deploy-backups"
FILES=("favicon.svg","robots.txt","sitemap.xml","status.json","docs/index.html","index.html")

def digest(p:Path):
    return hashlib.sha256(p.read_bytes()).hexdigest()

def safe_file(p:Path,may_be_absent:bool):
    if p.is_symlink():
        raise ValueError("Symlink prohibited: "+str(p))
    if not p.exists():
        if may_be_absent:return
        raise ValueError("Missing file: "+str(p))
    m=p.stat()
    if not stat.S_ISREG(m.st_mode) or m.st_uid!=os.getuid():
        raise ValueError("Not a private owned regular file: "+str(p))

def run():
    parser=argparse.ArgumentParser()
    parser.add_argument("--deploy",action="store_true",help="Publish after backup")
    args=parser.parse_args()
    subprocess.run([sys.executable,str(ROOT/"scripts/verify_official_site.py")],check=True)
    if not DEST.is_dir() or DEST.is_symlink() or DEST.stat().st_uid!=os.getuid():
        raise RuntimeError("Unexpected website directory/owner")
    for name in FILES:
        safe_file(SRC/name,False)
        to=DEST/name
        if to.parent.exists() and (not to.parent.is_dir() or to.parent.is_symlink()):
            raise ValueError("Unsafe target directory")
        safe_file(to,True)
        print(("CHANGE " if to.exists() else "CREATE ")+name+" sha256="+digest(SRC/name))
    if not args.deploy:
        print("DRY_RUN=PASS; no files modified")
        return
    backup=BACKUPS/(time.strftime("%Y%m%d-%H%M%S")+"-"+str(os.getpid()))
    backup.mkdir(parents=True,mode=0o700,exist_ok=False)
    try:
        # Back up ALL existing targets before changing any of them.
        for name in FILES:
            to=DEST/name
            if to.exists():
                old=backup/name
                old.parent.mkdir(parents=True,exist_ok=True)
                shutil.copy2(to,old,follow_symlinks=False)
                if digest(old)!=digest(to):raise RuntimeError("Backup checksum mismatch")
        for name in FILES:
            to=DEST/name
            to.parent.mkdir(parents=True,exist_ok=True)
            safe_file(to,True)
            staged=to.with_name("."+to.name+".stage-"+str(os.getpid()))
            if staged.exists() or staged.is_symlink():raise RuntimeError("Staging already exists")
            try:
                with staged.open("xb") as fp:
                    fp.write((SRC/name).read_bytes())
                    fp.flush()
                    os.fsync(fp.fileno())
                staged.chmod(0o644)
                if digest(staged)!=digest(SRC/name):raise RuntimeError("Stage hash mismatch")
                os.replace(staged,to)
                if digest(to)!=digest(SRC/name):raise RuntimeError("Live hash mismatch")
            finally:
                if staged.exists():staged.unlink()
    except Exception:
        print("PARTIAL DEPLOY POSSIBLE; saved backups at "+str(backup),file=sys.stderr)
        raise
    print("DEPLOY=PASS; backup="+str(backup))

if __name__=="__main__":run()
