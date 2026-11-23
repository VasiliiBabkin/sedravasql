use sedravasql::native_log::NativeLog;
use tempfile::tempdir;

#[test]
fn sqlite_magic_does_not_open_as_native_log() {
    let temp = tempdir().unwrap();
    let sqlite = temp.path().join("existing.sqlite");
    let mut db = sedravasql::Database::init(&sqlite).unwrap();
    db.mutate(
        "hello",
        "key",
        sedravasql::Change::Put("untouched"),
        0,
        "sqlite-original-0001",
    )
    .unwrap();
    drop(db);
    assert!(NativeLog::open(&sqlite).is_err());
    let db = sedravasql::Database::open(&sqlite).unwrap();
    assert_eq!(
        db.read("hello", "key").unwrap().unwrap().value.as_deref(),
        Some("untouched")
    );
}
