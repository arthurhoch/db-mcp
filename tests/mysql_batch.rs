#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;

use db_mcp::config::Engine;
use serde_json::json;
use sqlx::{Connection, Executor, SqlSafeStr};

#[tokio::test]
#[ignore = "Dedicated MySQL container; DB_MCP_INTEGRATION=1"]
async fn non_transactional_tables_are_rejected_before_batch_writes() {
    let fixture = common::Fixture::network(Engine::Mysql);
    let service = fixture.service();
    let table = format!(
        "dbmcp_myisam_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    common::succeeded(
        &common::execute(
            &service,
            "ddl",
            &format!("CREATE TABLE {table}(id INTEGER) ENGINE=MyISAM"),
        )
        .await,
    );
    let result=common::request(&service,json!({"database_id":"rw","statements":[{"sql":format!("INSERT INTO {table}(id) VALUES(1)")}]})).await;
    assert_eq!(result["error"]["code"], "BATCH_NOT_ATOMIC", "{result}");
    assert_eq!(result["execution_state"], "not_started");
    let observed = common::execute(&service, "ro", &format!("SELECT COUNT(*) FROM {table}")).await;
    assert_eq!(observed["rows"][0][0], 0);
    common::succeeded(&common::execute(&service, "ddl", &format!("DROP TABLE {table}")).await);
}

#[tokio::test]
#[ignore = "Dedicated MySQL container; DB_MCP_INTEGRATION=1"]
async fn triggered_tables_are_rejected_before_batch_writes() {
    let fixture = common::Fixture::network(Engine::Mysql);
    let service = fixture.service();
    let table = format!(
        "dbmcp_trigger_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    common::succeeded(
        &common::execute(
            &service,
            "ddl",
            &format!("CREATE TABLE {table}(id INTEGER) ENGINE=InnoDB"),
        )
        .await,
    );

    // Fixture setup uses the native driver: creating triggers is outside MCP policy.
    let options = sqlx::mysql::MySqlConnectOptions::new()
        .host("127.0.0.1")
        .port(53306)
        .database("dbmcp_test")
        .username("root")
        .password("disposable-root-password");
    let mut connection = sqlx::MySqlConnection::connect_with(&options).await.unwrap();
    let trigger_sql = format!(
        "CREATE TRIGGER {table}_before BEFORE INSERT ON {table} FOR EACH ROW SET NEW.id=NEW.id+1"
    );
    connection
        .execute(sqlx::AssertSqlSafe(trigger_sql).into_sql_str())
        .await
        .unwrap();
    connection.close().await.unwrap();

    let result = common::request(
        &service,
        json!({
            "database_id": "rw",
            "statements": [{"sql": format!("INSERT INTO {table}(id) VALUES(1)")}]
        }),
    )
    .await;
    assert_eq!(result["error"]["code"], "BATCH_NOT_ATOMIC", "{result}");
    assert_eq!(result["execution_state"], "not_started");
    let observed = common::execute(&service, "ro", &format!("SELECT COUNT(*) FROM {table}")).await;
    assert_eq!(observed["rows"][0][0], 0);
    common::succeeded(&common::execute(&service, "ddl", &format!("DROP TABLE {table}")).await);
}

#[tokio::test]
#[ignore = "Dedicated MySQL container; DB_MCP_INTEGRATION=1"]
async fn failed_myisam_statement_does_not_claim_rollback_of_partial_effects() {
    let fixture = common::Fixture::network(Engine::Mysql);
    let service = fixture.service();
    let table = format!(
        "dbmcp_partial_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    common::succeeded(
        &common::execute(
            &service,
            "ddl",
            &format!("CREATE TABLE {table}(id INTEGER PRIMARY KEY) ENGINE=MyISAM"),
        )
        .await,
    );
    let failed = common::execute(
        &service,
        "rw",
        &format!("INSERT INTO {table}(id) VALUES(1),(1)"),
    )
    .await;
    assert_eq!(failed["execution_state"], "unknown", "{failed}");
    assert_eq!(failed["effect"], "unknown");
    let observed = common::execute(&service, "ro", &format!("SELECT COUNT(*) FROM {table}")).await;
    assert_eq!(
        observed["rows"][0][0], 1,
        "Native MyISAM retains the first row: {observed}"
    );
    common::succeeded(&common::execute(&service, "ddl", &format!("DROP TABLE {table}")).await);
}
