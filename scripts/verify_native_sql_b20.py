#!/usr/bin/env python3
import json,tempfile,sqlite3
from pathlib import Path
from verify_native_index_b9 import run,sha
def main():
 with tempfile.TemporaryDirectory(prefix="sedravasql-b20-") as t:
    folder=Path(t);db=folder/"journal";run("init",db)
    run("sql",db,"CREATE TABLE Products (id TEXT PRIMARY KEY, title TEXT)","b20-products-001")
    run("sql",db,"CREATE TABLE Orders (id TEXT PRIMARY KEY, product TEXT, amount INT)","b20-orders-00001")
    run("sql",db,"ALTER TABLE Orders ADD FOREIGN KEY (product) REFERENCES Products(id)","b20-fk-0000001")
    tx=folder/"tx.json"
    data=[{"sql":"INSERT INTO Products (id,title) VALUES (?,?)","params":["p1","Молоко"]},{"sql":"INSERT INTO Orders (id,product,amount) VALUES (?,?,?)","params":["o1","p1",200]}]
    tx.write_text(json.dumps(data));first=run("sql-transaction",db,"b20-tx-0000001",tx)
    assert first["affected"]==2 and not first["replayed"]
    before=sha(db)
    assert run("sql-transaction",db,"b20-tx-0000001",tx)=={**first,"replayed":True}
    run("sql",db,"DELETE FROM Products WHERE id='p1'","b20-invalid-0001",fail=True)
    tx.write_text(json.dumps([{"sql":"INSERT INTO Products (id,title) VALUES ('p2','Tea')"},{"sql":"INSERT INTO Orders (id,product,amount) VALUES ('o2','missing',1)"}]))
    run("sql-transaction",db,"b20-invalid-0002",tx,fail=True)
    assert sha(db)==before
    assert run("sql",db,"SELECT id FROM Products")["rows"]==[["p1"]]
    assert run("sql",db,"SELECT Orders.id,Products.title FROM Orders JOIN Products ON Orders.product=Products.id")["rows"]==[["o1","Молоко"]]
    tx.write_text(json.dumps([{"sql":"DELETE FROM Products WHERE id='p1'"},{"sql":"DELETE FROM Orders WHERE id='o1'"}]))
    assert run("sql-transaction",db,"b20-delete-00001",tx)["affected"]==2
    run("verify",db)
    print("PASS B20: atomic bound multi-statement writes, foreign keys, rejection without journal change, parent/child delete, durable replay")
if __name__=="__main__":main()
