#!/usr/bin/env python3
import json,tempfile
from pathlib import Path
from verify_native_index_b9 import run,sha
def main():
 with tempfile.TemporaryDirectory(prefix="sedravasql-pages-") as t:
    f=Path(t);db=f/"log";snap=f/"snap";image=f/"pages";ops=f/"ops.json"
    run("init",db)
    for batch in range(10):
        ops.write_text(json.dumps([{"namespace":"test","key":f"k{batch*16+i:03}","expected_revision":0,"change":{"kind":"Put","value":f"value-{batch*16+i}"}} for i in range(16)]))
        run("batch",db,f"page-batch-{batch:06}",ops)
    original=sha(db);run("snapshot-create",db,snap,original)
    sh=sha(snap);report=run("page-create",snap,image,sh)
    assert report["rows"]==160 and report["height"]==3 and report["page_size"]==4096
    assert run("page-verify",image)==report
    for n in [0,11,12,143,144,159,160]:
        row=run("page-get",image,"test",f"k{n:03}")
        assert row["pages_read_after_open"]<=4
        assert row["row"]==({"namespace":"test","key":f"k{n:03}","revision":1,"value":f"value-{n}"} if n<160 else None)
    assert run("page-match-log",image,db)["matches"]
    assert sha(db)==original and sha(snap)==sh
    ops.write_text(json.dumps([{"namespace":"test","key":"k000","expected_revision":1,"change":{"kind":"Put","value":"new"}}]))
    run("batch",db,"page-change-0001",ops)
    assert not run("page-match-log",image,db)["matches"]
    assert run("page-get",image,"test","k000")["row"]["value"]=="value-0"
    before=sha(image);run("page-create",snap,image,sh,fail=True);assert sha(image)==before
    bad=f/"bad";data=bytearray(image.read_bytes());data[4096+16]^=1;bad.write_bytes(data);bad.chmod(0o600);run("page-verify",bad,fail=True)
    print(f"PASS B21/B22: {report['rows']} rows, {report['pages']} pages, height {report['height']}, <=4 page reads after verification, stale-generation detection and corruption rejection")
if __name__=="__main__":main()
