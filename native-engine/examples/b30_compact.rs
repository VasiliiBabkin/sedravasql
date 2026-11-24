#[cfg(unix)]
fn main() {
    use sedravasql_native_engine::cow_tree::{CowTree, Mutation, Record};
    use serde_json::{json, Value};
    use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path, time::Instant};
    fn ms(t: Instant) -> f64 {
        t.elapsed().as_secs_f64() * 1000.
    }
    fn metrics(mut a: Vec<f64>) -> Value {
        a.sort_by(f64::total_cmp);
        let n = a.len();
        json!({"n":n,"median_ms":(a[(n-1)/2]+a[n/2])/2.0,"p95_ms":a[(n*95).div_ceil(100)-1],"samples_ms":a})
    }
    fn key(i: usize) -> String {
        format!("k{i:05}")
    }
    fn reads(t: &mut CowTree, oracle: &BTreeMap<String, Record>) -> Value {
        let n = oracle.len();
        let mut get = vec![];
        let mut range = vec![];
        for i in 0..245 {
            let k = key(i * 7919 % n);
            let start = Instant::now();
            let result = t.get(&k).unwrap();
            let elapsed = ms(start);
            assert_eq!(result.as_ref(), oracle.get(&k));
            if i >= 5 {
                get.push(elapsed);
            }
            let k = i * 7919 % (n - 8 + 1);
            let a = key(k);
            let b = key(k + 8);
            let start = Instant::now();
            let result = t.range(&a, &b, 8).unwrap();
            let elapsed = ms(start);
            assert_eq!(
                result,
                oracle
                    .range(a..b)
                    .map(|(_, v)| v.clone())
                    .collect::<Vec<_>>()
            );
            if i >= 5 {
                range.push(elapsed);
            }
        }
        json!({"get":metrics(get),"range8":metrics(range)})
    }
    fn remove_fixture(path: &Path) {
        fs::remove_file(path).unwrap();
        fs::remove_file(path.with_extension("head")).unwrap();
    }
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut path = d.path().join("source0");
    let mut t = CowTree::create(&path).unwrap();
    let mut oracle = BTreeMap::new();
    let mut results = vec![];
    // 5 independently verified growth cycles, then churn at the last size.
    for phase in 0..6 {
        let start = Instant::now();
        let mut commits = vec![];
        if phase < 5 {
            for i in (0..4096).step_by(16) {
                let ops = (i..i + 16)
                    .map(|j| {
                        let n = phase * 4096 + j * 7919 % 4096;
                        let k = key(n);
                        let v = format!("value-{n:05}");
                        oracle.insert(
                            k.clone(),
                            Record {
                                key: k.clone(),
                                revision: 1,
                                value: Some(v.clone()),
                            },
                        );
                        Mutation {
                            key: k,
                            expected_revision: 0,
                            value: Some(v),
                        }
                    })
                    .collect::<Vec<_>>();
                let a = Instant::now();
                t.commit(&ops).unwrap();
                commits.push(ms(a));
            }
        } else {
            for i in (0..4096).step_by(16) {
                let ops = (i..i + 16)
                    .map(|j| {
                        let n = j * 7919 % 20480;
                        let k = key(n);
                        let value = if j % 5 == 0 {
                            None
                        } else {
                            Some("Ж".repeat(128))
                        };
                        let old = oracle.get_mut(&k).unwrap();
                        let rev = old.revision;
                        old.revision += 1;
                        old.value = value.clone();
                        Mutation {
                            key: k,
                            expected_revision: rev,
                            value,
                        }
                    })
                    .collect::<Vec<_>>();
                let a = Instant::now();
                t.commit(&ops).unwrap();
                commits.push(ms(a));
            }
        }
        let write_ms = ms(start);
        let before = t.report();
        let before_reads = reads(&mut t, &oracle);
        let dest = d.path().join(format!("packed{}", phase + 1));
        let a = Instant::now();
        let packed = t.compact_copy(&dest).unwrap();
        let compact_ms = ms(a);
        assert_eq!(packed.rows, before.rows);
        assert_eq!(packed.generation, before.generation);
        assert_eq!(fs::metadata(&path).unwrap().len() + 4096, before.file_bytes);
        drop(t);
        let a = Instant::now();
        t = CowTree::open(&dest).unwrap();
        let reopen_ms = ms(a);
        let after_reads = reads(&mut t, &oracle);
        assert_eq!(t.report().rows, oracle.len() as u64);
        // Exhaustive validation of every current key, including tombstones.
        let a = Instant::now();
        for (k, v) in &oracle {
            assert_eq!(t.get(k).unwrap().as_ref(), Some(v));
        }
        let full_oracle_ms = ms(a);
        results.push(json!({"phase":phase+1,"kind":if phase<5{"insert"}else{"churn"},"rows":oracle.len(),"live_rows":oracle.values().filter(|r|r.value.is_some()).count(),"generation":before.generation,"write_ms":write_ms,"commit16":metrics(commits),"before":before,"packed":packed,"compact_ms":compact_ms,"reopen_ms":reopen_ms,"full_oracle_ms":full_oracle_ms,"before_reads":before_reads,"after_reads":after_reads}));
        eprintln!(
            "phase {}: {} keys, {} -> {} bytes; compact {:.1} ms",
            phase + 1,
            oracle.len(),
            before.file_bytes,
            packed.file_bytes,
            compact_ms
        );
        // Only disposable test fixtures; the compact_copy API itself
        // never changes or deletes the original.
        remove_fixture(&path);
        path = dest;
    }
    println!(
        "{}",
        json!({"schema":"b30-compact-growth-v1","notes":"Six phases of 256 atomic 16-op batches, 20480 keys then 4096 updates/deletes. Copy into independent packed destination; reopen and exhaustively verify all physical keys. 240 point and range8 samples per phase before and after. Source deletion only in disposable benchmark after complete verification.","records":results})
    );
}
#[cfg(not(unix))]
fn main() {
    panic!("Unix only");
}
