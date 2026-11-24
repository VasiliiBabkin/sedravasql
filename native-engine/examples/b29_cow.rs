#[cfg(unix)]
fn main() {
    use sedravasql_native_engine::{
        cow_tree::{CowTree, Mutation},
        tree::Tree,
    };
    use serde_json::{json, Value};
    use std::{fs, os::unix::fs::PermissionsExt, time::Instant};
    fn metrics(mut samples: Vec<f64>) -> Value {
        samples.sort_by(f64::total_cmp);
        let n = samples.len();
        json!({"n":n,"median_ms":(samples[(n-1)/2]+samples[n/2])/2.,"p95_ms":samples[(n*95).div_ceil(100)-1],"samples_ms":samples})
    }
    fn elapsed(t: Instant) -> f64 {
        t.elapsed().as_secs_f64() * 1000.
    }
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut records = vec![];
    for n in [128usize, 512, 2048] {
        for variant in if n == 2048 {
            vec!["cow-single", "cow-batch16"]
        } else {
            vec![
                "b25",
                "cow-single",
                "cow-batch16",
                "cow-batch16",
                "cow-single",
                "b25",
            ]
        } {
            let p = d.path().join(format!("fixture-{}", records.len()));
            let mut inserts = vec![];
            let mut gets = vec![];
            let mut ranges = vec![];
            let start = Instant::now();
            let (build_ms, open_ms, bytes, report) = if variant == "b25" {
                let mut t = Tree::create(&p, 1024).unwrap();
                for i in 0..n {
                    let k = i * 7919 % n;
                    let a = Instant::now();
                    t.put(&format!("k{k:05}"), 0, Some(format!("value-{k:05}")))
                        .unwrap();
                    inserts.push(elapsed(a));
                }
                let build_ms = elapsed(start);
                drop(t);
                let a = Instant::now();
                let mut t = Tree::open(&p).unwrap();
                let open_ms = elapsed(a);
                for i in 0..245 {
                    let k = i * 37 % n;
                    let key = format!("k{k:05}");
                    let a = Instant::now();
                    let row = t.get(&key).unwrap().unwrap();
                    let ms = elapsed(a);
                    assert_eq!(row.value, Some(format!("value-{k:05}")));
                    assert_eq!(row.revision, 1);
                    if i >= 5 {
                        gets.push(ms);
                    }
                    let k = i * 37 % (n - 8 + 1);
                    let a = Instant::now();
                    let rows = t
                        .range(&format!("k{k:05}"), &format!("k{:05}", k + 8), 8)
                        .unwrap();
                    let ms = elapsed(a);
                    assert_eq!(
                        rows.iter().map(|r| (&r.key, &r.value)).collect::<Vec<_>>(),
                        (k..k + 8)
                            .map(|x| (format!("k{x:05}"), Some(format!("value-{x:05}"))))
                            .collect::<Vec<_>>()
                            .iter()
                            .map(|(k, v)| (k, v))
                            .collect::<Vec<_>>()
                    );
                    if i >= 5 {
                        ranges.push(ms);
                    }
                }
                (
                    build_ms,
                    open_ms,
                    fs::metadata(&p).unwrap().len(),
                    json!(t.report()),
                )
            } else {
                let mut t = CowTree::create(&p).unwrap();
                let batch = if variant == "cow-single" { 1 } else { 16 };
                for i in (0..n).step_by(batch) {
                    let ops = (i..i + batch)
                        .map(|j| {
                            let k = j * 7919 % n;
                            Mutation {
                                key: format!("k{k:05}"),
                                expected_revision: 0,
                                value: Some(format!("value-{k:05}")),
                            }
                        })
                        .collect::<Vec<_>>();
                    let a = Instant::now();
                    t.commit(&ops).unwrap();
                    inserts.push(elapsed(a));
                }
                let build_ms = elapsed(start);
                drop(t);
                let a = Instant::now();
                let mut t = CowTree::open(&p).unwrap();
                let open_ms = elapsed(a);
                for i in 0..245 {
                    let k = i * 37 % n;
                    let key = format!("k{k:05}");
                    let a = Instant::now();
                    let row = t.get(&key).unwrap().unwrap();
                    let ms = elapsed(a);
                    assert_eq!(row.value, Some(format!("value-{k:05}")));
                    assert_eq!(row.revision, 1);
                    if i >= 5 {
                        gets.push(ms);
                    }
                    let k = i * 37 % (n - 8 + 1);
                    let a = Instant::now();
                    let rows = t
                        .range(&format!("k{k:05}"), &format!("k{:05}", k + 8), 8)
                        .unwrap();
                    let ms = elapsed(a);
                    assert_eq!(
                        rows.into_iter()
                            .map(|r| (r.key, r.value))
                            .collect::<Vec<_>>(),
                        (k..k + 8)
                            .map(|x| (format!("k{x:05}"), Some(format!("value-{x:05}"))))
                            .collect::<Vec<_>>()
                    );
                    if i >= 5 {
                        ranges.push(ms);
                    }
                }
                (build_ms, open_ms, t.report().file_bytes, json!(t.report()))
            };
            records.push(json!({"rows":n,"variant":variant,"build_ms":build_ms,"open_ms":open_ms,"bytes":bytes,"report":report,"commit":metrics(inserts),"get":metrics(gets),"range8":metrics(ranges)}));
            fs::remove_file(&p).unwrap();
            if variant != "b25" {
                fs::remove_file(p.with_extension("head")).unwrap();
            }
        }
    }
    println!(
        "{}",
        json!({"schema":"b29-cow-benchmark-v1","notes":"B25 fixed 1024 slots vs COW one row/commit and atomic 16-row batches; per-commit samples not per-row for batch16. Reopen validates current tree. 5 read warmups, 240 timed point and 8-row range queries per case. 2048 only COW; no SQL or SQLite comparison.","records":records})
    );
}
#[cfg(not(unix))]
fn main() {
    panic!("Unix only");
}
