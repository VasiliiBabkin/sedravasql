#!/usr/bin/env python3
import sqlite3,tempfile
from pathlib import Path
from verify_native_index_b9 import run,sha
def main():
 with tempfile.TemporaryDirectory(prefix="sedravasql-b18-") as t:
    db=Path(t)/"journal";run("init",db);o=sqlite3.connect(":memory:")
    commands=["CREATE TABLE Sales (id TEXT PRIMARY KEY, item INT, amount INT)","CREATE TABLE Catalog (id TEXT PRIMARY KEY, item INT, title TEXT)"]
    commands += [f"INSERT INTO Sales (id,item,amount) VALUES ('s{i:02}',{i%6},{i})" for i in range(18)]
    commands += [f"INSERT INTO Catalog (id,item,title) VALUES ('c{i:02}',{i%4},'item{i}')" for i in range(8)]
    for i,q in enumerate(commands):run("sql",db,q,f"join-setup-{i:04}");o.execute(q)
    before=sha(db)
    queries=["SELECT Sales.id,Catalog.title FROM Sales JOIN Catalog ON Sales.item=Catalog.item ORDER BY Sales.id",
    "SELECT Sales.id,Catalog.title FROM Sales LEFT JOIN Catalog ON Sales.item=Catalog.item ORDER BY Sales.id",
    "SELECT Sales.id,Catalog.id FROM Sales INNER JOIN Catalog ON Catalog.item=Sales.item WHERE Sales.amount>=9 ORDER BY Sales.id DESC LIMIT 7",
    "SELECT Sales.id,Catalog.title FROM Sales LEFT OUTER JOIN Catalog ON Sales.item=Catalog.item WHERE Catalog.item=2 ORDER BY Sales.id",
    "SELECT Sales.id,Catalog.id FROM Sales LEFT JOIN Catalog ON Sales.item=Catalog.item ORDER BY Catalog.id LIMIT 8"]
    for q in queries:
        got=run("sql",db,q);expected=[list(r) for r in o.execute(q).fetchall()]
        assert got["kind"]=="join_rows" and got["rows"]==expected,(q,got,expected)
    assert run("explain",db,queries[0])["access"]=="query_local_right_index"
    assert sha(db)==before
    print("PASS B18: 26 source rows, 5 SQLite JOIN oracle comparisons, null extension, multiplicity, filtering, sorting, limit and unchanged journal")
if __name__=="__main__":main()
