//! B36: compare the same public transaction contract, not bare SQLite SQL.
#[cfg(unix)]
mod bench {
    use sedravasql::{
        native_log::{
            cow_store::CowStore, NativeChange, NativeLog, NativeMutation, TransactionalStore,
        },
        sqlite_batch::SqliteBatchStore,
    };
    use serde_json::{json, Value};
    use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Instant};
    const WIDTH: usize = 4;
    fn open(kind: &str, p: &Path, create: bool) -> Box<dyn TransactionalStore> {
        match kind {
            "cow" => Box::new(
                if create {
                    CowStore::create(p)
                } else {
                    CowStore::open(p)
                }
                .unwrap(),
            ),
            "native-log" => Box::new(
                if create {
                    NativeLog::create(p)
                } else {
                    NativeLog::open(p)
                }
                .unwrap(),
            ),
            "sqlite-batch" => Box::new(
                if create {
                    SqliteBatchStore::init(p)
                } else {
                    SqliteBatchStore::open(p)
                }
                .unwrap(),
            ),
            _ => panic!("backend"),
        }
    }
    fn key(i: usize) -> String {
        format!("k{i:06}")
    }
    fn payload(i: usize, rev: u64) -> String {
        format!("{i:06}-{rev}-{}", "x".repeat(119))
    }
    fn ops(batch: usize, rev: u64) -> Vec<NativeMutation> {
        (batch * WIDTH..(batch + 1) * WIDTH)
            .map(|i| NativeMutation {
                namespace: "bench".into(),
                key: key(i),
                expected_revision: rev - 1,
                change: NativeChange::Put(payload(i, rev)),
            })
            .collect()
    }
    fn elapsed(t: Instant) -> f64 {
        t.elapsed().as_secs_f64() * 1000.
    }
    fn stats(mut xs: Vec<f64>) -> Value {
        xs.sort_by(f64::total_cmp);
        let n = xs.len();
        json!({"n":n,"median_ms":(xs[(n-1)/2]+xs[n/2])/2.,"p95_ms":xs[(n*95).div_ceil(100)-1],"sum_ms":xs.iter().sum::<f64>(),"min_ms":xs[0],"max_ms":xs[n-1]})
    }
    fn size(dir: &Path) -> u64 {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                if e.file_type().unwrap().is_file() {
                    e.metadata().unwrap().len()
                } else {
                    0
                }
            })
            .sum()
    }
    fn run(kind: &str, batches: usize, rep: usize) -> Value {
        let d = tempfile::tempdir().unwrap();
        fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let p = d.path().join("db");
        let t = Instant::now();
        let mut db = open(kind, &p, true);
        let create = elapsed(t);
        let mut insert = vec![];
        let mut update = vec![];
        let mut read = vec![];
        let mut replay = vec![];
        let mut reopen = vec![];
        for b in 0..batches {
            let o = ops(b, 1);
            let k = format!("insert-{b:08}");
            let t = Instant::now();
            let r = db.commit_batch(&k, &o).unwrap();
            insert.push(elapsed(t));
            assert_eq!(r.sequence, b as u64 + 1);
            assert!(!r.replayed);
            assert_eq!(r.entries.len(), WIDTH);
            for (i, e) in r.entries.iter().enumerate() {
                assert_eq!(e.revision, 1);
                assert_eq!(e.value.as_deref(), Some(payload(b * WIDTH + i, 1).as_str()));
            }
        }
        for b in 0..batches / 2 {
            let o = ops(b, 2);
            let k = format!("update-{b:08}");
            let t = Instant::now();
            let r = db.commit_batch(&k, &o).unwrap();
            update.push(elapsed(t));
            assert_eq!(r.sequence, (batches + b + 1) as u64);
            assert!(!r.replayed);
            assert_eq!(r.entries.len(), WIDTH);
            assert!(r.entries.iter().all(|e| e.revision == 2));
        }
        let live = size(d.path());
        for i in 0..1024 {
            let n = (i * 37 + rep * 13) % (batches * WIDTH);
            let k = key(n);
            let t = Instant::now();
            let r = db.read("bench", &k).unwrap().unwrap();
            read.push(elapsed(t));
            let rev = if n < batches / 2 * WIDTH { 2 } else { 1 };
            assert_eq!(r.revision, rev);
            assert_eq!(r.value.as_deref(), Some(payload(n, rev).as_str()));
        }
        for i in 0..128 {
            let b = (i * 17 + rep * 7) % batches;
            let o = ops(b, 1);
            let k = format!("insert-{b:08}");
            let t = Instant::now();
            let r = db.commit_batch(&k, &o).unwrap();
            replay.push(elapsed(t));
            assert!(r.replayed);
            assert_eq!(r.sequence, b as u64 + 1);
            for (i, e) in r.entries.iter().enumerate() {
                assert_eq!(e.revision, 1);
                assert_eq!(e.value.as_deref(), Some(payload(b * WIDTH + i, 1).as_str()));
            }
        }
        assert!(db.commit_batch("insert-00000000", &ops(0, 2)).is_err());
        drop(db);
        let closed = size(d.path());
        for _ in 0..5 {
            let t = Instant::now();
            let reopened = open(kind, &p, false);
            reopen.push(elapsed(t));
            let r = reopened.read("bench", &key(0)).unwrap().unwrap();
            assert_eq!(r.revision, 2);
            drop(reopened);
        }
        let mut db = open(kind, &p, false);
        let old = db.commit_batch("insert-00000000", &ops(0, 1)).unwrap();
        assert!(old.replayed);
        assert_eq!(old.sequence, 1);
        for i in 0..batches * WIDTH {
            let row = db.read("bench", &key(i)).unwrap().unwrap();
            let rev = if i < batches / 2 * WIDTH { 2 } else { 1 };
            assert_eq!(row.revision, rev);
            assert_eq!(row.value.as_deref(), Some(payload(i, rev).as_str()));
        }
        drop(db);
        json!({"backend":kind,"batches":batches,"rep":rep,"rows":batches*WIDTH,"width":WIDTH,"transactions":batches+batches/2,
        "create_ms":create,"insert":stats(insert),"update":stats(update),"read":stats(read),"replay":stats(replay),"reopen":stats(reopen),"live_bytes":live,"closed_bytes":closed,"verified":true})
    }
    pub fn main() {
        let backends = ["native-log", "cow", "sqlite-batch"];
        for batches in [64, 256] {
            for rep in 0..3 {
                for j in 0..3 {
                    let kind = backends[(j + rep) % 3];
                    println!("{}", run(kind, batches, rep));
                }
            }
        }
    }
}
#[cfg(unix)]
fn main() {
    bench::main();
}
#[cfg(not(unix))]
fn main() {
    eprintln!("B36 requires Unix");
    std::process::exit(1);
}
