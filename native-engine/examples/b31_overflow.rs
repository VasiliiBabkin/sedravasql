#[cfg(unix)]
fn main() {
    use sedravasql_native_engine::cow_tree::{CowTree, Mutation, Record};
    use serde_json::{json, Value};
    use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, time::Instant};
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
    fn value(i: usize, len: usize) -> String {
        let prefix = format!("{i:05}:");
        prefix.chars().cycle().take(len).collect()
    }
    fn read(t: &mut CowTree, oracle: &BTreeMap<String, Record>) -> Value {
        let n = oracle.len();
        let mut point: BTreeMap<usize, Vec<f64>> = BTreeMap::new();
        let mut range = vec![];
        // Deterministic mixed workload. 5 warmups per point-size class.
        for (class, len) in [64, 257, 4000, 4001, 16384, 65536].into_iter().enumerate() {
            for i in 0..85 {
                let k = key(n / 4 + (i * 37 % ((n - n / 4) / 6)) * 6 + class);
                let start = Instant::now();
                let result = t.get(&k).unwrap();
                let elapsed = ms(start);
                assert_eq!(result.as_ref(), oracle.get(&k));
                if i >= 5 {
                    point.entry(len).or_default().push(elapsed);
                }
            }
        }
        for i in 0..125 {
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
        json!({"point_by_bytes":point.into_iter().map(|(k,v)|(k,metrics(v))).collect::<BTreeMap<_,_>>(),"range8":metrics(range)})
    }
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut results = vec![];
    for n in [192usize, 960] {
        let p = d.path().join(format!("db{n}"));
        let q = d.path().join(format!("packed{n}"));
        let mut t = CowTree::create(&p).unwrap();
        let mut oracle = BTreeMap::new();
        let mut commits = vec![];
        let start = Instant::now();
        for base in (0..n).step_by(16) {
            let ops = (base..base + 16)
                .map(|i| {
                    let k = key(i);
                    let v = value(i, [64, 257, 4000, 4001, 16384, 65536][i % 6]);
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
        let insert_ms = ms(start);
        let mut change = vec![];
        for base in (0..n / 4).step_by(16) {
            let ops = (base..base + 16)
                .map(|i| {
                    let k = key(i);
                    let v = match i % 3 {
                        0 => None,
                        1 => Some(value(i, 65536)),
                        _ => Some(value(i, 64)),
                    };
                    let old = oracle.get_mut(&k).unwrap();
                    let rev = old.revision;
                    old.revision += 1;
                    old.value = v.clone();
                    Mutation {
                        key: k,
                        expected_revision: rev,
                        value: v,
                    }
                })
                .collect::<Vec<_>>();
            let a = Instant::now();
            t.commit(&ops).unwrap();
            change.push(ms(a));
        }
        let before = t.report();
        let payload: u64 = oracle
            .values()
            .map(|r| r.value.as_ref().map_or(0, |v| v.len() as u64))
            .sum();
        let a = Instant::now();
        let packed = t.compact_copy(&q).unwrap();
        let compact_ms = ms(a);
        assert_eq!(packed.generation, before.generation);
        drop(t);
        let a = Instant::now();
        let mut t = CowTree::open(&q).unwrap();
        let open_ms = ms(a);
        for (k, v) in &oracle {
            assert_eq!(t.get(k).unwrap().as_ref(), Some(v));
        }
        // Point samples use the unchanged suffix, so size classes are exact.
        let reads = read(&mut t, &oracle);
        results.push(json!({"keys":n,"live_keys":oracle.values().filter(|r|r.value.is_some()).count(),"payload_bytes":payload,"before":before,"packed":packed,"insert_total_ms":insert_ms,"insert16":metrics(commits),"change16":metrics(change),"compact_ms":compact_ms,"open_ms":open_ms,"reads":reads}));
        eprintln!(
            "{n} keys: {payload} value bytes, compact {} -> {} bytes",
            before.file_bytes, packed.file_bytes
        );
    }
    // Exercise the explicit 32-MiB compact-copy budget in release mode.
    let p = d.path().join("budget");
    let q = d.path().join("budget-copy");
    let mut t = CowTree::create(&p).unwrap();
    let v = "x".repeat(65536);
    for base in (0..512).step_by(16) {
        let ops = (base..base + 16)
            .map(|i| Mutation {
                key: key(i),
                expected_revision: 0,
                value: Some(v.clone()),
            })
            .collect::<Vec<_>>();
        t.commit(&ops).unwrap();
    }
    let a = Instant::now();
    let exact = t.compact_copy(&q).unwrap();
    let exact_ms = ms(a);
    assert_eq!(exact.rows, 512);
    t.commit(&[Mutation {
        key: key(512),
        expected_revision: 0,
        value: Some("x".into()),
    }])
    .unwrap();
    let rejected = d.path().join("over-budget");
    let before = t.report();
    let error = t.compact_copy(&rejected).unwrap_err();
    assert!(matches!(
        error,
        sedravasql_native_engine::Error::Capacity(_)
    ));
    assert!(!rejected.exists() && !rejected.with_extension("head").exists());
    assert_eq!(before.generation, t.report().generation);
    assert_eq!(
        t.get(&key(512)).unwrap().unwrap().value.as_deref(),
        Some("x")
    );
    println!(
        "{}",
        json!({"schema":"b31-overflow-bench-v1","notes":"One release run, mixed-size values, exact full oracle after compact/reopen, 80 point samples per value-size class and 120 range8; no comparison to SQLite. Explicit snapshot budget at 32 MiB succeeds, 32 MiB + 1 rejects before destination creation.","records":results,"snapshot_budget":{"exact_bytes":33554432,"exact_packed_bytes":exact.file_bytes,"compact_ms":exact_ms,"over_budget_bytes":33554433,"rejected":true}})
    );
}
#[cfg(not(unix))]
fn main() {
    panic!("Unix only");
}
