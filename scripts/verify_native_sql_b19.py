#!/usr/bin/env python3
import sqlite3,tempfile
from pathlib import Path
from verify_native_index_b9 import run,sha
def main():
 with tempfile.TemporaryDirectory(prefix="sedravasql-b19-") as t:
    db=Path(t)/"journal";run("init",db);oracle=sqlite3.connect(":memory:")
    create="CREATE TABLE Stock (id TEXT PRIMARY KEY, category TEXT, amount INT)"
    run("sql",db,create,"index-demo-create");oracle.execute(create)
    for i in range(30):
        q=f"INSERT INTO Stock (id,category,amount) VALUES ('s{i:02}','c{i%5}',{i})"
        run("sql",db,q,f"index-demo-{i:04}");oracle.execute(q)
    qs=["SELECT id,amount FROM Stock WHERE category='c2' ORDER BY id",
        "SELECT DISTINCT category FROM Stock ORDER BY category DESC LIMIT 3",
        "SELECT category,SUM(amount) AS total FROM Stock GROUP BY category HAVING total>=80 ORDER BY total DESC",
        "SELECT COUNT(*) AS n FROM Stock WHERE category='c2' HAVING n>4",
        "SELECT AVG(amount) AS mean FROM Stock HAVING mean>=14"]
    def compare():
        before=sha(db)
        for q in qs:
            expected=[list(r) for r in oracle.execute(q).fetchall()]
            got=run("sql",db,q)
            assert got["rows"]==expected,(q,got,expected)
        assert sha(db)==before
    compare()
    assert run("explain",db,qs[0])["access"]=="bounded_table_scan"
    run("sql",db,"CREATE INDEX stock_category ON Stock(category)","index-demo-create2")
    oracle.execute("CREATE INDEX stock_category ON Stock(category)")
    plan=run("explain",db,qs[0]);assert plan["access"]=="secondary_index_lookup" and plan["index_name"]=="stock_category" and not plan["index_cached"]
    compare()
    for i,q in enumerate(["UPDATE Stock SET category='c2' WHERE id='s00'","DELETE FROM Stock WHERE id='s02'"]):
        run("sql",db,q,f"index-demo-change-{i:04}");oracle.execute(q)
    compare()
    print("PASS B19: 30 rows, 15 SQLite oracle comparisons before/after index and updates; durable index definition, DISTINCT, HAVING")
if __name__=="__main__":main()
