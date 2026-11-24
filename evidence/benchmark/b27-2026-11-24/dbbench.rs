use sedravasql_native_engine::{NativeLog,sql::{execute,execute_bound,SqlResult},table::Cell};
use rusqlite::{Connection,params};
use serde_json::{json,Value};
use std::{path::Path,time::Instant,fs,collections::BTreeMap};
fn timed<T>(f:impl FnOnce()->T)->(T,f64){let t=Instant::now();let v=f();(v,t.elapsed().as_secs_f64()*1000.)}
fn metrics(mut v:Vec<f64>)->Value{v.sort_by(|a,b|a.total_cmp(b));let n=v.len();json!({"n":n,"median_ms":if n%2==0{(v[n/2-1]+v[n/2])/2.}else{v[n/2]},"p95_ms":v[(n*95+99)/100-1],"total_ms":v.iter().sum::<f64>(),"samples_ms":v})}
fn rows(r:SqlResult)->Value{match r{SqlResult::Rows{rows,..}=>json!(rows),SqlResult::AggregateRows{rows,..}|SqlResult::JoinRows{rows,..}=>json!(rows),_=>panic!("expected rows")}}
fn sqlite_insert(db:&mut Connection,i:usize){
 let k=format!("p{i:04}");let cat=format!("c{}",i%8);let title=format!("Product {i}");let price=i as i32;
 let request=format!("bench-insert-{i:06}");
 let tx=db.transaction().unwrap();
 tx.execute("INSERT INTO Products (id,title,category,price) VALUES (?,?,?,?)",params![k,title,cat,price]).unwrap();
 // Persist an operation receipt in the same durable transaction. The native engine
 // additionally performs hash-chaining, receipt/CAS checks and replay validation.
 tx.execute("INSERT INTO Receipts (request_key,operation) VALUES (?,?)",params![request,json!([k,title,cat,price]).to_string()]).unwrap();
 tx.commit().unwrap();
}
fn native_insert(db:&mut NativeLog,i:usize){
 let p=[Cell::Text(format!("p{i:04}")),Cell::Text(format!("Product {i}")),Cell::Text(format!("c{}",i%8)),Cell::Int(i as i32)];
 execute_bound(db,"INSERT INTO Products (id,title,category,price) VALUES (?,?,?,?)",&p,Some(&format!("bench-insert-{i:06}"))).unwrap();
}
fn query_sqlite(db:&Connection,q:&str,p:&[&dyn rusqlite::ToSql])->Value{
 let mut st=db.prepare(q).unwrap();let n=st.column_count();
 let vals=st.query_map(p,|row|{let mut v=vec![];for i in 0..n{v.push(match row.get_ref(i)?{rusqlite::types::ValueRef::Integer(x)=>json!(x),rusqlite::types::ValueRef::Text(x)=>json!(std::str::from_utf8(x).unwrap()),rusqlite::types::ValueRef::Null=>Value::Null,_=>panic!("unexpected")})}Ok(v)}).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
 json!(vals)
}
fn main(){
 let args=std::env::args().collect::<Vec<_>>();let dir=Path::new(&args[1]);fs::create_dir_all(dir).unwrap();
 let mut output=vec![];
 for round in 0..3{
   let sd=dir.join(format!("round{round}"));fs::create_dir(&sd).unwrap();
   let native=sd.join("native.log");let sqlite=sd.join("sqlite.db");
   let mut n=NativeLog::create(&native).unwrap();let mut s=Connection::open(&sqlite).unwrap();
   s.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE Products(id TEXT PRIMARY KEY,title TEXT NOT NULL,category TEXT NOT NULL,price INT NOT NULL);CREATE INDEX products_category ON Products(category);CREATE TABLE Receipts(request_key TEXT PRIMARY KEY,operation TEXT NOT NULL);").unwrap();
   execute(&mut n,"CREATE TABLE Products (id TEXT PRIMARY KEY,title TEXT,category TEXT,price INT)",Some("bench-schema-0001")).unwrap();
   execute(&mut n,"CREATE INDEX products_category ON Products(category)",Some("bench-schema-0002")).unwrap();
   for i in 0..100{native_insert(&mut n,i);sqlite_insert(&mut s,i);}
   let mut a=BTreeMap::<String,Vec<f64>>::new();let mut b=BTreeMap::<String,Vec<f64>>::new();
   for i in 100..300{
      if (i+round)%2==0{a.entry("insert".into()).or_default().push(timed(||native_insert(&mut n,i)).1);b.entry("insert".into()).or_default().push(timed(||sqlite_insert(&mut s,i)).1);}
      else{b.entry("insert".into()).or_default().push(timed(||sqlite_insert(&mut s,i)).1);a.entry("insert".into()).or_default().push(timed(||native_insert(&mut n,i)).1);}
   }
   let pk="SELECT id,title,price FROM Products WHERE id=?";
   let category="SELECT id,title,price FROM Products WHERE category=? ORDER BY id LIMIT 100";
   let sum="SELECT COUNT(*) AS n,SUM(price) AS total FROM Products WHERE category=?";
   let scan="SELECT id,title,price FROM Products WHERE title=? ORDER BY id LIMIT 100";
   for (kind,q,reps) in [("pk",pk,300),("category",category,150),("aggregate",sum,150),("scan",scan,100)]{
      // warm the index, and use identical results, not just status codes.
      for i in 0..reps+3{
        let key=if kind=="pk"{format!("p{:04}",(i*37)%300)}else if kind=="scan"{format!("Product {}",(i*37)%300)}else{format!("c{}",i%8)};
        let nf=|db:&mut NativeLog|rows(execute_bound(db,q,&[Cell::Text(key.clone())],None).unwrap());
        let sf=||query_sqlite(&s,q,&[&key]);
        let ((nr,nt),(sr,st))=if (i+round)%2==0{(timed(||nf(&mut n)),timed(sf))}else{let x=timed(sf);(timed(||nf(&mut n)),x)};
        assert_eq!(nr,sr,"{kind} {key}");
        if i>=3{a.entry(kind.into()).or_default().push(nt);b.entry(kind.into()).or_default().push(st);}
      }
   }
   n.verify().unwrap();assert_eq!(s.query_row("SELECT COUNT(*) FROM Products",[],|r|r.get::<_,i64>(0)).unwrap(),300);
   assert_eq!(s.query_row("PRAGMA integrity_check",[],|r|r.get::<_,String>(0)).unwrap(),"ok");
   let live=fs::metadata(&sqlite).unwrap().len()+fs::metadata(sqlite.with_extension("db-wal")).map(|x|x.len()).unwrap_or(0);
   s.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();drop(s);drop(n);
   output.push(json!({"round":round,"native":a.into_iter().map(|(k,v)|(k,metrics(v))).collect::<BTreeMap<_,_>>(),"sqlite":b.into_iter().map(|(k,v)|(k,metrics(v))).collect::<BTreeMap<_,_>>(),"native_bytes":fs::metadata(native).unwrap().len(),"sqlite_bytes_checkpointed":fs::metadata(sqlite).unwrap().len(),"sqlite_bytes_live_with_wal":live}));
 }
 println!("{}",json!({"rounds":output,"sqlite_version":rusqlite::version(),"rows":300,"durability":"native fsync / SQLite WAL synchronous FULL, receipt per insert (not identical semantics)","notes":"In-process Rust APIs; includes SQL parse, result materialization to JSON values; no process start; native extra checks, durable historical receipts. Journal vs checkpointed SQLite different retention."}));
}
