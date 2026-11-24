#![cfg(unix)]
use sedravasql_native_engine::{
    sql::{build_tree_projection, execute, execute_bound, SqlResult, TreeSqlReader},
    table::Cell,
    NativeLog,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, time::Instant};

fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let result = f();
    (result, t.elapsed().as_secs_f64() * 1000.)
}
fn metrics(mut v: Vec<f64>) -> Value {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    json!({"n":n,"median_ms":(v[(n-1)/2]+v[n/2])/2.,"p95_ms":v[(n*95).div_ceil(100)-1],"samples_ms":v})
}
fn main() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut records = Vec::new();
    let query = "SELECT id,n FROM T WHERE id>=? AND id<? ORDER BY id";
    for size in [32usize, 128, 512] {
        let log = d.path().join(format!("log-{size}"));
        let path = d.path().join(format!("tree-{size}"));
        let mut db = NativeLog::create(&log).unwrap();
        execute(
            &mut db,
            "CREATE TABLE T (id TEXT PRIMARY KEY, n INT)",
            Some("bench-schema-0001"),
        )
        .unwrap();
        for i in 0..size {
            execute(
                &mut db,
                &format!("INSERT INTO T (id,n) VALUES ('k{i:04}',{i})"),
                Some(&format!("bench-insert-{i:04}")),
            )
            .unwrap();
        }
        let (report, build_ms) = timed(|| build_tree_projection(&mut db, "T", &path).unwrap());
        let before = fs::read(&log).unwrap();
        let cases: Vec<_> = [1usize, 8, 32]
            .into_iter()
            .flat_map(|width| {
                (0..120).map(move |i| {
                    let start = (i * 37) % (size - width + 1);
                    let params = [
                        Cell::Text(format!("k{start:04}")),
                        Cell::Text(format!("k{:04}", start + width)),
                    ];
                    let expected = SqlResult::Rows {
                        columns: vec!["id".into(), "n".into()],
                        rows: (start..start + width)
                            .map(|n| vec![Cell::Text(format!("k{n:04}")), Cell::Int(n as i32)])
                            .collect(),
                    };
                    (width, params, expected)
                })
            })
            .collect();
        let mut samples: BTreeMap<&str, BTreeMap<usize, Vec<f64>>> = BTreeMap::new();
        let mut attach = Vec::new();
        for variant in ["scan", "tree", "tree", "scan"] {
            if variant == "scan" {
                for (_, p, e) in cases.iter().take(5) {
                    assert_eq!(&execute_bound(&mut db, query, p, None).unwrap(), e);
                }
                for (width, p, e) in &cases {
                    let (r, ms) = timed(|| execute_bound(&mut db, query, p, None).unwrap());
                    assert_eq!(&r, e);
                    samples
                        .entry(variant)
                        .or_default()
                        .entry(*width)
                        .or_default()
                        .push(ms);
                }
            } else {
                let (mut reader, ms) = timed(|| TreeSqlReader::open(&mut db, "T", &path).unwrap());
                attach.push(ms);
                for (_, p, e) in cases.iter().take(5) {
                    assert_eq!(&reader.query(query, p).unwrap(), e);
                }
                for (width, p, e) in &cases {
                    let (r, ms) = timed(|| reader.query(query, p).unwrap());
                    assert_eq!(&r, e);
                    samples
                        .entry(variant)
                        .or_default()
                        .entry(*width)
                        .or_default()
                        .push(ms);
                }
            }
        }
        assert_eq!(fs::read(&log).unwrap(), before);
        db.verify().unwrap();
        records.push(json!({"rows":size,"build_ms":build_ms,"attach_ms":attach,"tree_report":report,"log_bytes":fs::metadata(&log).unwrap().len(),"tree_bytes":fs::metadata(&path).unwrap().len(),"metrics":samples.into_iter().map(|(k,v)|(k,v.into_iter().map(|(width,values)|(width,metrics(values))).collect::<BTreeMap<_,_>>())).collect::<BTreeMap<_,_>>() }));
    }
    println!(
        "{}",
        json!({"schema":"b28-range-bench-v1","order":["scan","tree","tree","scan"],"notes":"In-process same SQL parser and result checks. Build and full verification/attach timed separately. 120 queries per width per phase, 5 initial warmups; one shared Linux host.","records":records})
    );
}
