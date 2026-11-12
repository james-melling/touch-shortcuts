//! `schema/tsp-v2.schema.json` is generated from the validator's bounds.
//! Regenerate with `TSP_UPDATE_SCHEMA=1 cargo test -p tsp-core --test schema`.

use std::path::Path;
use serde_json::Value;
use tsp_core::profile::{to_text, v2_json_schema};

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

#[test]
fn schema_file_matches_the_validator() {
    let path = root().join("schema/tsp-v2.schema.json");
    let want = to_text(&v2_json_schema());
    if std::env::var("TSP_UPDATE_SCHEMA").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &want).unwrap();
    }
    let got = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(got == want, "schema/tsp-v2.schema.json is stale; run TSP_UPDATE_SCHEMA=1 cargo test -p tsp-core --test schema");
}

/// Minimal draft-07 check for the keywords the generated schema uses
/// (type, const, enum, required, properties, items, bounds, lengths).
fn check(schema: &Value, v: &Value, at: &str, errors: &mut Vec<String>) {
    let s = schema.as_object().unwrap();
    if let Some(t) = s.get("type").and_then(Value::as_str) {
        let ok = match t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "integer" => v.is_u64() || v.is_i64(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            _ => true,
        };
        if !ok {
            errors.push(format!("{at}: expected {t}"));
            return;
        }
    }
    if let Some(c) = s.get("const") {
        if c != v {
            errors.push(format!("{at}: expected {c}"));
        }
    }
    if let Some(Value::Array(e)) = s.get("enum") {
        if !e.contains(v) {
            errors.push(format!("{at}: {v} not in enum"));
        }
    }
    if let (Some(n), Some(min)) = (v.as_f64(), s.get("minimum").and_then(Value::as_f64)) {
        if n < min {
            errors.push(format!("{at}: {n} < {min}"));
        }
    }
    if let (Some(n), Some(max)) = (v.as_f64(), s.get("maximum").and_then(Value::as_f64)) {
        if n > max {
            errors.push(format!("{at}: {n} > {max}"));
        }
    }
    if let Some(text) = v.as_str() {
        let len = text.chars().count() as u64;
        if s.get("minLength").and_then(Value::as_u64).is_some_and(|m| len < m) || s.get("maxLength").and_then(Value::as_u64).is_some_and(|m| len > m) {
            errors.push(format!("{at}: length {len}"));
        }
    }
    if let (Some(obj), Some(Value::Array(req))) = (v.as_object(), s.get("required")) {
        for r in req {
            if !obj.contains_key(r.as_str().unwrap()) {
                errors.push(format!("{at}: missing {r}"));
            }
        }
    }
    if let (Some(obj), Some(Value::Object(props))) = (v.as_object(), s.get("properties")) {
        for (k, sub) in props {
            if let Some(child) = obj.get(k) {
                check(sub, child, &format!("{at}.{k}"), errors);
            }
        }
    }
    if let (Some(items), Some(arr)) = (s.get("items"), v.as_array()) {
        if s.get("maxItems").and_then(Value::as_u64).is_some_and(|m| arr.len() as u64 > m) {
            errors.push(format!("{at}: too many items"));
        }
        for (i, item) in arr.iter().enumerate() {
            check(items, item, &format!("{at}[{i}]"), errors);
        }
    }
}

#[test]
fn schema_accepts_every_shipped_v2_profile_and_rejects_a_bad_one() {
    let schema = v2_json_schema();
    let mut count = 0;
    for entry in std::fs::read_dir(root().join("profiles")).unwrap() {
        let p = entry.unwrap().path();
        let doc: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        if doc["schema"] != 2 {
            continue;
        }
        count += 1;
        let mut errors = Vec::new();
        check(&schema, &doc, "$", &mut errors);
        assert!(errors.is_empty(), "{}: {errors:?}", p.display());
    }
    assert!(count >= 1);
    let bad = serde_json::json!({"schema": 2, "name": "T", "pad": {"grid": {"columns": 13, "rows": 8}, "cellWidth": 69, "cellHeight": 76, "gap": 8}});
    let mut errors = Vec::new();
    check(&schema, &bad, "$", &mut errors);
    assert_eq!(errors, vec!["$.pad.grid.columns: 13 > 12".to_string()]);
}
