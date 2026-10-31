#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;

use std::{fs, sync::Arc};

use db_mcp::execution::ExecutionService;
use serde_json::json;

#[tokio::test]
async fn an_unrepresentable_row_returns_an_explicit_limit_error() {
    let fixture = common::Fixture::sqlite();
    let mut catalog = fixture.catalog();
    Arc::make_mut(catalog.databases.get_mut("ro").unwrap())
        .limits
        .max_result_bytes = 1024;
    let service = ExecutionService::new(catalog);
    let result = common::execute(&service, "ro", "SELECT hex(zeroblob(4096))").await;
    assert_eq!(result["error"]["code"], "LIMIT_EXCEEDED", "{result}");
    assert_eq!(result["effect"], "none");
}

#[tokio::test]
async fn envelope_error_after_commit_preserves_the_known_committed_effect() {
    let fixture = common::Fixture::sqlite();
    let mut catalog = fixture.catalog();
    Arc::make_mut(catalog.databases.get_mut("rw").unwrap())
        .limits
        .max_result_bytes = 1024;
    let service = ExecutionService::new(catalog);
    common::succeeded(
        &common::execute(&service, "ddl", "CREATE TABLE committed(id INTEGER)").await,
    );
    let statements = (0..20)
        .map(|id| json!({"sql":format!("INSERT INTO committed(id) VALUES({id})")}))
        .collect::<Vec<_>>();
    let result = common::request(
        &service,
        json!({"database_id":"rw","statements":statements}),
    )
    .await;
    assert_eq!(result["error"]["code"], "SERIALIZATION_ERROR", "{result}");
    assert_eq!(result["effect"], "committed");
    let observed = common::execute(&service, "ro", "SELECT COUNT(*) FROM committed").await;
    assert_eq!(observed["rows"][0][0], 20);
}

#[tokio::test]
async fn concurrent_calls_keep_independent_destinations() {
    let first = common::Fixture::sqlite();
    let second = common::Fixture::sqlite();
    let second_path = second.directory.path().join("databases/ddl.toml");
    let mut second_config: toml::Value =
        toml::from_str(&fs::read_to_string(&second_path).unwrap()).unwrap();
    second_config["id"] = "second".into();
    second_config["connection"]["url"] = format!(
        "sqlite:{}",
        second.directory.path().join("sample.db").display()
    )
    .into();
    fs::write(
        first.directory.path().join("databases/second.toml"),
        toml::to_string(&second_config).unwrap(),
    )
    .unwrap();
    let service = first.service();
    let (one, two) = tokio::join!(
        common::execute(&service, "ddl", "CREATE TABLE only_first(id INTEGER)"),
        common::execute(&service, "second", "CREATE TABLE only_second(id INTEGER)")
    );
    common::succeeded(&one);
    common::succeeded(&two);
    let first_tables = common::execute(
        &service,
        "ro",
        "SELECT name FROM sqlite_schema WHERE type='table'",
    )
    .await;
    assert_eq!(first_tables["rows"], json!([["only_first"]]));
    let second_tables = common::execute(
        &service,
        "second",
        "SELECT name FROM sqlite_schema WHERE type='table'",
    )
    .await;
    assert_eq!(second_tables["rows"], json!([["only_second"]]));
}

#[test]
fn decimal_exponents_cannot_expand_into_unbounded_strings() {
    let parameter: db_mcp::model::Parameter =
        serde_json::from_value(json!({"type":"decimal","value":"1e999999999"})).unwrap();
    assert!(parameter.resolve().is_err());
}
