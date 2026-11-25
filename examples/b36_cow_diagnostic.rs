//! Diagnostic only: raw COW pages, not equivalent SQL backends.
#[cfg(unix)]
mod bench {
    use sedravasql::native_log::cow_tree::{CowTree, Mutation};
    use serde_json::json;
    use std::{fs, os::unix::fs::PermissionsExt, time::Instant};
    pub fn main() {
        for rep in 0..3 {
            for j in 0..2 {
                let reuse = (rep + j) % 2 == 0;
                let dir = tempfile::tempdir().unwrap();
                fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
                let p = dir.path().join("tree");
                let mut tree = CowTree::create(&p).unwrap();
                let mut windows = vec![];
                for i in 0..384 {
                    let k = format!("frame-{i:06}");
                    let ops = [
                        Mutation {
                            key: k.clone(),
                            expected_revision: 0,
                            value: Some("x".repeat(700)),
                        },
                        Mutation {
                            key: format!("{k}-desc"),
                            expected_revision: 0,
                            value: Some("y".repeat(100)),
                        },
                    ];
                    let t = Instant::now();
                    if reuse {
                        tree.commit_reusing(&ops)
                    } else {
                        tree.commit(&ops)
                    }
                    .unwrap();
                    windows.push(t.elapsed().as_secs_f64() * 1000.);
                }
                let pages = tree.report().pages;
                drop(tree);
                let mut tree = CowTree::open(&p).unwrap();
                for i in 0..384 {
                    let k = format!("frame-{i:06}");
                    assert_eq!(tree.get(&k).unwrap().unwrap().value, Some("x".repeat(700)));
                    assert_eq!(
                        tree.get(&format!("{k}-desc")).unwrap().unwrap().value,
                        Some("y".repeat(100))
                    );
                }
                let median = |xs: &[f64]| {
                    let mut x = xs.to_vec();
                    x.sort_by(f64::total_cmp);
                    (x[47] + x[48]) / 2.
                };
                println!(
                    "{}",
                    json!({"rep":rep,"mode":if reuse{"verified-reuse"}else{"append-only"},"windows_median_ms":windows.chunks(96).map(median).collect::<Vec<_>>(),"total_ms":windows.iter().sum::<f64>(),"data_pages":pages,"verified":true})
                );
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
