#!/usr/bin/env python3
"""Independent sqlite oracle for supported SQL subset; native engine executes all tested queries."""
import sqlite3,tempfile
from pathlib import Path
from verify_native_index_b9 import run,sha
def main():
    with tempfile.TemporaryDirectory(prefix="sedravasql-b16b-") as t:
        log=Path(t)/"journal";run("init",log)
        oracle=sqlite3.connect(":memory:")
        create="CREATE TABLE Orders (id TEXT PRIMARY KEY, amount INT, paid BOOL, note TEXT)"
        run("sql",log,create,"sql-create-00001");oracle.execute(create)
        for i in range(24):
            sql=f"INSERT INTO Orders (id,amount,paid,note) VALUES ('o{i:02}',{i%7},{'true' if i%2 else 'false'},'тест ''{i}')"
            run("sql",log,sql,f"sql-insert-{i:06}");oracle.execute(sql)
        def compare(q):
            cur=oracle.execute(q);columns=[x[0] for x in cur.description]
            expected=[list(r) for r in cur.fetchall()]
            # SQLite represents bool as 0/1, native JSON bool compares equivalently in Python.
            got=run("sql",log,q)
            assert got=={"kind":"rows","columns":columns,"rows":expected},(q,got,expected)
        queries=["SELECT id,amount FROM Orders WHERE amount>=3 ORDER BY amount DESC LIMIT 10",
            "SELECT id,paid,note FROM Orders WHERE id='o07'",
            "SELECT * FROM Orders WHERE paid=true AND amount<>0 ORDER BY id ASC",
            "SELECT id,amount FROM Orders WHERE amount<2 ORDER BY amount ASC LIMIT 12",
            "SELECT * FROM Orders WHERE id='absent'"]
        # SQLite equal-sort tie ordering here follows insertion/PK order.
        for q in queries:compare(q)
        update="UPDATE Orders SET amount=-5,paid=true WHERE amount<2"
        receipt=run("sql",log,update,"sql-update-00001");assert receipt["affected"]==oracle.execute(update).rowcount
        delete="DELETE FROM Orders WHERE id='o07'"
        assert run("sql",log,delete,"sql-delete-00001")["affected"]==oracle.execute(delete).rowcount
        for q in queries:compare(q)
        before=sha(log)
        retry=run("sql",log,update,"sql-update-00001");assert retry=={**receipt,"replayed":True}
        run("sql",log,"UPDATE Orders SET amount=0 WHERE amount>=-5","sql-too-many-001",fail=True)
        run("sql",log,"UPDATE Orders SET amount='wrong' WHERE id='absent'","sql-invalid-0001",fail=True)
        run("sql",log,"DELETE FROM Orders","sql-invalid-0002",fail=True)
        assert sha(log)==before
        print("PASS B16b: 24 rows, 10 independent SQLite query comparisons, atomic UPDATE/DELETE, durable replay, strict errors and unchanged journal")
if __name__=="__main__":main()
