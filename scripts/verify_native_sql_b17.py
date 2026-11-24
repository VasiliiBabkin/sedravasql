#!/usr/bin/env python3
import sqlite3,tempfile
from pathlib import Path
from verify_native_index_b9 import run,sha
def main():
    with tempfile.TemporaryDirectory(prefix="sedravasql-b17-") as t:
        db=Path(t)/"journal";run("init",db);oracle=sqlite3.connect(":memory:")
        q="CREATE TABLE Sales (id TEXT PRIMARY KEY, category TEXT, amount INT, paid BOOL)"
        run("sql",db,q,"create-sales-001");oracle.execute(q)
        for i in range(36):
            q=f"INSERT INTO Sales (id,category,amount,paid) VALUES ('s{i:02}','c{i%3}',{i-9},{'true' if i%2 else 'false'})"
            run("sql",db,q,f"insert-sales-{i:04}");oracle.execute(q)
        before=sha(db)
        queries=["SELECT COUNT(*) AS n,SUM(amount) AS total,AVG(amount) AS mean,MIN(amount) AS lo,MAX(amount) AS hi FROM Sales",
        "SELECT category,COUNT(*) AS n,SUM(amount) AS total FROM Sales GROUP BY category ORDER BY category",
        "SELECT category,AVG(amount) AS mean FROM Sales WHERE paid=true GROUP BY category ORDER BY mean DESC LIMIT 2",
        "SELECT COUNT(id) AS n,SUM(amount) AS total FROM Sales WHERE id='missing'",
        "SELECT category,COUNT(*) AS n FROM Sales WHERE amount< -999 GROUP BY category",
        "SELECT MIN(category) AS lo,MAX(category) AS hi FROM Sales",
        "SELECT paid,COUNT(*) AS n FROM Sales GROUP BY paid ORDER BY paid"]
        for q in queries:
            cursor=oracle.execute(q);expected=[list(x) for x in cursor.fetchall()]
            got=run("sql",db,q)
            assert got=={"kind":"aggregate_rows","columns":[x[0] for x in cursor.description],"rows":expected},(q,got,expected)
        assert run("explain",db,"SELECT * FROM Sales WHERE id='s00'")["access"]=="primary_key_lookup"
        assert run("explain",db,"SELECT category,SUM(amount) AS s FROM Sales GROUP BY category ORDER BY s")["access"]=="bounded_table_scan"
        assert sha(db)==before
        print("PASS B17: 36 rows, 7 aggregate/group SQLite oracle comparisons, primary-key vs scan plan, unchanged journal")
if __name__=="__main__":main()
