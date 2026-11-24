use sedravasql_native_engine::{
    sql::{execute, execute_bound, explain, SqlResult},
    table::Cell,
    NativeLog,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, time::Instant};

fn metrics(mut v: Vec<f64>) -> Value {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    json!({"n":n,"median_ms":(v[(n-1)/2]+v[n/2])/2.,"p95_ms":v[(n*95).div_ceil(100)-1],"samples_ms":v})
}
#[cfg(not(unix))]
fn main() {
    panic!("NativeLog requires Unix");
}
#[cfg(unix)]
fn main() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let q = "SELECT id,n FROM T WHERE id>=? AND id<? ORDER BY id";
    let scan = "SELECT id,n FROM T WHERE id>=? AND id>=? AND id<? ORDER BY id";
    let mut records = vec![];
    for size in [32usize, 128, 512, 2048] {
        let path = d.path().join(format!("log-{size}"));
        let mut db = NativeLog::create(&path).unwrap();
        execute(
            &mut db,
            "CREATE TABLE T (id TEXT PRIMARY KEY,n INT)",
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
        let before = fs::read(&path).unwrap();
        let mut samples: BTreeMap<&str, BTreeMap<usize, Vec<f64>>> = BTreeMap::new();
        for width in [1usize, 8, 32] {
            for i in 0..245 {
                let start = (i * 37) % (size - width + 1);
                let p = [
                    Cell::Text(format!("k{start:04}")),
                    Cell::Text(format!("k{:04}", start + width)),
                ];
                let full = [p[0].clone(), p[0].clone(), p[1].clone()];
                if i == 0 {
                    assert_eq!(explain(&mut db, q, &p).unwrap().access, "primary_key_range");
                    assert_eq!(
                        explain(&mut db, scan, &full).unwrap().access,
                        "bounded_table_scan"
                    );
                }
                let expected = SqlResult::Rows {
                    columns: vec!["id".into(), "n".into()],
                    rows: (start..start + width)
                        .map(|n| vec![Cell::Text(format!("k{n:04}")), Cell::Int(n as i32)])
                        .collect(),
                };
                let order = if i % 2 == 0 {
                    ["scan", "range"]
                } else {
                    ["range", "scan"]
                };
                for variant in order {
                    let t = Instant::now();
                    let result = if variant == "range" {
                        execute_bound(&mut db, q, &p, None)
                    } else {
                        execute_bound(&mut db, scan, &full, None)
                    }
                    .unwrap();
                    let ms = t.elapsed().as_secs_f64() * 1000.;
                    assert_eq!(result, expected);
                    if i >= 5 {
                        samples
                            .entry(variant)
                            .or_default()
                            .entry(width)
                            .or_default()
                            .push(ms);
                    }
                }
            }
        }
        assert_eq!(before, fs::read(&path).unwrap());
        db.verify().unwrap();
        records.push(json!({"rows":size,"log_bytes":before.len(),"metrics":samples.into_iter().map(|(k,v)|(k,v.into_iter().map(|(w,v)|(w,metrics(v))).collect::<BTreeMap<_,_>>())).collect::<BTreeMap<_,_>>() }));
    }
    println!(
        "{}",
        json!({"schema":"b28-ordered-range-bench-v1","notes":"One process, alternating per pair. Same result and semantically equivalent SQL; scan includes redundant lower bound to force legacy plan (extra predicate/parser cost included). Five warmups per width. No disk tree or SQLite.","records":records})
    );
}
