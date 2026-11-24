#!/usr/bin/env python3
"""Validate the public static SedravaSQL website without network or credentials."""
from __future__ import annotations
import json
import re
from pathlib import Path
from html.parser import HTMLParser
from xml.etree import ElementTree

SITE=Path(__file__).resolve().parents[1]/"site"
FILES={"index.html","docs/index.html","favicon.svg","robots.txt","sitemap.xml","status.json","README.md"}
class Audit(HTMLParser):
    def __init__(self):
        super().__init__();self.ids=set();self.links=[];self.canonical=[];self.h1=0
    def handle_starttag(self,tag,attrs):
        a=dict(attrs)
        if a.get("id"):
            assert a["id"] not in self.ids,("duplicate id",a["id"])
            self.ids.add(a["id"])
        if tag=="a" and a.get("href"):self.links.append(a["href"])
        if tag=="link" and a.get("rel")=="canonical":self.canonical.append(a.get("href"))
        if tag=="h1":self.h1+=1
def check_html(path:Path,canonical:str):
    text=path.read_text("utf-8")
    assert "FASTPANEL" not in text.upper()
    assert not re.search(r"(?i)(api.?token|secret_key|password)\s*[:=]\s*['\"][^'\"]{12,}",text)
    a=Audit();a.feed(text)
    assert a.h1==1,(path,a.h1)
    assert a.canonical==[canonical],(path,a.canonical)
    for link in a.links:
        assert not link.startswith(("javascript:","//"))
        if link.startswith("https://"):
            assert link.startswith(("https://github.com/","https://sql.virtreal.ru/")),link
            continue
        if link.startswith("#"):
            assert link[1:] in a.ids,(path,link)
            continue
        assert link.startswith("/"),(path,link)
        dest=SITE/link.lstrip("/")
        if link.endswith("/"):dest=dest/"index.html"
        assert dest.is_file(),(path,link)
    print("PASS",path.relative_to(SITE),"links",len(a.links))
def main():
    found={str(x.relative_to(SITE)) for x in SITE.rglob("*") if x.is_file()}
    assert found==FILES,(found-FILES,FILES-found)
    check_html(SITE/"index.html","https://sql.virtreal.ru/")
    check_html(SITE/"docs/index.html","https://sql.virtreal.ru/docs/")
    status=json.loads((SITE/"status.json").read_text("utf-8"))
    assert status["production_ready"] is False
    assert status["current_public_alpha"]=="0.1.0-alpha.1"
    assert status["experimental_native_engine"]=="B28"
    assert status["research_stage"]=="B31"
    assert status["research_commit"]=="a98e709cc591057298818d0972c12cf020054295"
    assert status["basis_commit"]=="363ccbc9542ed8e3ecf41e3619f78823ffeb97d4"
    root=ElementTree.parse(SITE/"sitemap.xml")
    links=sorted(node.text for node in root.iter() if node.tag.endswith("loc"))
    assert links==["https://sql.virtreal.ru/","https://sql.virtreal.ru/docs/"]
    assert "Sitemap:" in (SITE/"robots.txt").read_text()
    print("PASS sitemap, status.json, robots; no embedded secrets")
if __name__=="__main__":main()
