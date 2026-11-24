use sedravasql_native_engine::table::Cell;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Previous {
    Int(i32),
    Bool(bool),
    Text(String),
}
impl From<Previous> for Cell {
    fn from(v: Previous) -> Self {
        match v {
            Previous::Int(v) => Self::Int(v),
            Previous::Bool(v) => Self::Bool(v),
            Previous::Text(v) => Self::Text(v),
        }
    }
}
#[test]
fn scalar_json_dispatch_matches_previous_untagged_decoder() {
    let fixed = [
        "0",
        "-0",
        "1",
        "-1",
        "2147483647",
        "-2147483648",
        "2147483648",
        "-2147483649",
        "18446744073709551615",
        "18446744073709551616",
        "-9223372036854775809",
        "1.0",
        "1e0",
        "true",
        "false",
        "null",
        "[]",
        "{}",
        "[1]",
        "{\"Int\":1}",
        r#""hello""#,
        r#""""#,
        r#""1""#,
        r#""true""#,
        r#""Привет 🌍""#,
        r#""\uD83C\uDF0D""#,
        r#""\u0000""#,
        r#""\n\t\\\"""#,
        r#""\uD800""#,
        "01",
        "+1",
        "NaN",
        "true false",
    ];
    for source in fixed
        .into_iter()
        .map(str::to_owned)
        .chain((-1024..=1024).map(|i| i.to_string()))
    {
        let old = serde_json::from_str::<Previous>(&source)
            .map(Cell::from)
            .ok();
        let new = serde_json::from_str::<Cell>(&source).ok();
        assert_eq!(old, new, "input: {source}");
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&source) {
            let old_value = serde_json::from_value::<Previous>(value.clone())
                .map(Cell::from)
                .ok();
            let new_value = serde_json::from_value::<Cell>(value).ok();
            assert_eq!(old_value, new_value, "value: {source}");
        }
        if let Some(cell) = new {
            assert_eq!(
                serde_json::from_str::<Cell>(&serde_json::to_string(&cell).unwrap()).unwrap(),
                cell
            );
        }
    }
}
