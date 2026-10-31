#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;

use db_mcp::{
    config::Engine,
    model::{DescribeRequest, MetadataKind},
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

async fn database_contract(fixture: common::Fixture) {
    let service = fixture.service();
    let select_parameter = format!(
        "SELECT {} AS bound_integer{}",
        fixture.placeholder(1),
        if fixture.engine == Engine::Oracle {
            " FROM dual"
        } else {
            ""
        }
    );
    let bound = common::request(&service, json!({"database_id":"ro","sql":select_parameter,"parameters":[{"type":"integer","value":"42"}]})).await;
    common::succeeded(&bound);
    assert_eq!(number(&bound["rows"][0][0]), 42);
    let table = format!("dbmcp_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let id_type = if fixture.engine == Engine::Oracle {
        "NUMBER(20)"
    } else {
        "BIGINT"
    };
    let create = common::execute(&service, "ddl", &format!("CREATE TABLE {table}(id {id_type} PRIMARY KEY, amount DECIMAL(30,8), note VARCHAR(100))")).await;
    common::succeeded(&create);
    assert_eq!(create["effect"], "committed");

    if fixture.engine == Engine::Oracle {
        common::wait_for_oracle_ddl(&service, &table).await;
    }

    let summary = service
        .describe(
            DescribeRequest {
                database_id: "ro".into(),
                include: MetadataKind::Summary,
                schema: None,
                table: None,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        summary["version"].as_str().is_some_and(|v| !v.is_empty()),
        "{summary}"
    );

    let sql = format!(
        "INSERT INTO {table}(id,amount,note) VALUES ({},{},{})",
        fixture.placeholder(1),
        fixture.placeholder(2),
        fixture.placeholder(3)
    );
    let insert = common::request(&service, json!({"database_id":"rw","sql":sql,"parameters":[{"type":"integer","value":"9007199254740993"},{"type":"decimal","value":"12345678901234567890.12345678"},{"type":"string","value":"O'Reilly; DROP TABLE ignored"}]})).await;
    common::succeeded(&insert);
    assert_eq!(insert["affected_rows"], 1);

    let result = common::execute(
        &service,
        "ro",
        &format!("SELECT id AS duplicate_name,id AS duplicate_name,amount,note FROM {table}"),
    )
    .await;
    common::succeeded(&result);
    assert_eq!(result["columns"].as_array().unwrap().len(), 4);
    assert_eq!(result["rows"][0][0], "9007199254740993");
    assert_eq!(result["rows"][0][3], "O'Reilly; DROP TABLE ignored");
    if fixture.engine != Engine::Sqlite {
        assert_eq!(result["rows"][0][2], "12345678901234567890.12345678");
    }

    let denied = common::execute(&service, "ro", &format!("DELETE FROM {table}")).await;
    assert_eq!(denied["error"]["code"], "READ_ONLY_VIOLATION");
    let denied = common::execute(
        &service,
        "rw",
        &format!("ALTER TABLE {table} ADD other INTEGER"),
    )
    .await;
    assert_eq!(denied["error"]["code"], "OPERATION_NOT_SUPPORTED");

    let altered = common::execute(
        &service,
        "ddl",
        &format!("ALTER TABLE {table} ADD other INTEGER"),
    )
    .await;
    common::succeeded(&altered);
    if fixture.engine == Engine::Oracle {
        common::wait_for_oracle_ddl(&service, &table).await;
    }

    let batch = common::request(&service, json!({"database_id":"rw","statements":[{"sql":format!("INSERT INTO {table}(id,note) VALUES (2,'first')")},{"sql":format!("INSERT INTO {table}(id,note) VALUES (2,'duplicate')")}]})).await;
    assert_eq!(batch["effect"], "rolled_back", "{batch}");
    let after = common::execute(&service, "ro", &format!("SELECT COUNT(*) FROM {table}")).await;
    assert_eq!(number(&after["rows"][0][0]), 1, "{after}");

    let batch = common::request(&service, json!({"database_id":"rw","statements":[{"sql":format!("INSERT INTO {table}(id,note) VALUES (2,'second')")},{"sql":format!("INSERT INTO {table}(id,note) VALUES (3,'third')")}]})).await;
    common::succeeded(&batch);
    assert_eq!(batch["effect"], "committed");
    let limited = common::execute(
        &service,
        "ro",
        &format!("SELECT id FROM {table} ORDER BY id"),
    )
    .await;
    assert_eq!(limited["returned_rows"], 2, "{limited}");
    assert_eq!(limited["truncated"], true, "{limited}");

    let table_name = if fixture.engine == Engine::Oracle {
        table.to_uppercase()
    } else {
        table.clone()
    };
    let columns = service
        .describe(
            DescribeRequest {
                database_id: "ro".into(),
                include: MetadataKind::Columns,
                schema: None,
                table: Some(table_name),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(columns["returned_rows"], 2, "{columns}");
    assert_eq!(columns["truncated"], true);

    let update = common::execute(
        &service,
        "rw",
        &format!("UPDATE {table} SET note='changed' WHERE id=2"),
    )
    .await;
    common::succeeded(&update);
    let delete = common::execute(&service, "rw", &format!("DELETE FROM {table} WHERE id=2")).await;
    common::succeeded(&delete);
    let dropped = common::execute(&service, "ddl", &format!("DROP TABLE {table}")).await;
    common::succeeded(&dropped);
}

fn number(value: &Value) -> i64 {
    value
        .as_i64()
        .unwrap_or_else(|| value.as_str().unwrap().parse().unwrap())
}

#[tokio::test]
async fn sqlite_executes_reads_dml_ddl_and_transactional_batches() {
    database_contract(common::Fixture::sqlite()).await;
}

#[tokio::test]
#[ignore = "Dedicated PostgreSQL container; DB_MCP_INTEGRATION=1"]
async fn postgres_executes_reads_dml_ddl_and_transactional_batches() {
    database_contract(common::Fixture::network(Engine::Postgresql)).await;
}

#[tokio::test]
#[ignore = "Dedicated MySQL container; DB_MCP_INTEGRATION=1"]
async fn mysql_executes_reads_dml_ddl_and_transactional_batches() {
    database_contract(common::Fixture::network(Engine::Mysql)).await;
}

#[tokio::test]
#[ignore = "Dedicated Oracle container; DB_MCP_INTEGRATION=1"]
async fn oracle_executes_reads_dml_ddl_and_transactional_batches() {
    database_contract(common::Fixture::network(Engine::Oracle)).await;
}

#[tokio::test]
async fn sqlite_preserves_null_binary_and_duplicate_column_labels() {
    let fixture = common::Fixture::sqlite();
    let service = fixture.service();
    let result = common::request(&service, json!({"database_id":"ro","sql":"SELECT ?1 AS x,?2 AS x,?3 AS x","parameters":[{"type":"null"},{"type":"binary","value":"AP8="},{"type":"integer","value":"9007199254740993"}]})).await;
    common::succeeded(&result);
    assert_eq!(result["rows"], json!([[null, "AP8=", "9007199254740993"]]));
}

#[tokio::test]
async fn missing_destination_is_never_replaced_by_another_database() {
    let fixture = common::Fixture::sqlite();
    let result = common::execute(
        &fixture.service(),
        "does-not-exist",
        "CREATE TABLE leak(id INTEGER)",
    )
    .await;
    assert_eq!(result["error"]["code"], "DATABASE_NOT_AVAILABLE");
    assert_eq!(result["execution_state"], "not_started");
}

#[tokio::test]
async fn rejected_late_batch_statement_does_not_execute_earlier_ddl() {
    let fixture = common::Fixture::sqlite();
    let service = fixture.service();
    let result = common::request(&service, json!({"database_id":"ddl","statements":[{"sql":"CREATE TABLE must_not_exist(id INTEGER)"},{"sql":"PRAGMA writable_schema=ON"}]})).await;
    assert_eq!(result["execution_state"], "not_started");
    let check = common::execute(
        &service,
        "ro",
        "SELECT name FROM sqlite_schema WHERE name='must_not_exist'",
    )
    .await;
    assert_eq!(check["rows"], json!([]));
}

#[tokio::test]
async fn pre_cancelled_request_does_not_open_or_mutate_a_database() {
    let fixture = common::Fixture::sqlite();
    let token = CancellationToken::new();
    token.cancel();
    let input = serde_json::from_value(
        json!({"database_id":"ddl","sql":"CREATE TABLE cancelled(id INTEGER)"}),
    )
    .unwrap();
    let result = fixture.service().execute(input, token).await;
    assert_eq!(result["error"]["code"], "EXECUTION_CANCELLED");
    assert_eq!(result["effect"], "none");
}
