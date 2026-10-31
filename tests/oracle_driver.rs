#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;

use db_mcp::{config::Engine, execution::ExecutionService};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::test]
#[ignore = "Dedicated Oracle container; DB_MCP_INTEGRATION=1"]
async fn scalar_bindings_preserve_precision_nulls_bytes_and_timestamps() {
    let fixture = common::Fixture::network(Engine::Oracle);
    let result = common::request(
        &fixture.service(),
        json!({
            "database_id": "ro",
            "sql": "SELECT :1 AS n, :2 AS bytes_value, :3 AS decimal_value,
                       :4 AS small_decimal, :5 AS unsigned_value, :6 AS timestamp_value,
                       :7 AS utc_value, :8 AS date_value, :9 AS float_value,
                       :10 AS boolean_value, :11 AS text_value,
                       :12 AS zero_value, :13 AS smallest_number, :14 AS largest_number FROM dual",
            "parameters": [
                {"type":"null"},
                {"type":"binary","value":"AP8="},
                {"type":"decimal","value":"-12345678901234567890.12345678"},
                {"type":"decimal","value":"1e-12"},
                {"type":"integer","value":"18446744073709551615"},
                {"type":"timestamp","value":"2026-10-31T11:22:33.123456789"},
                {"type":"timestamp","value":"2026-10-31T11:22:33.987654321-03:00"},
                {"type":"date","value":"2026-10-31"},
                {"type":"float","value":2.5},
                {"type":"boolean","value":true},
                {"type":"string","value":"O'Reilly; SELECT ignored"},
                {"type":"decimal","value":"0"},
                {"type":"decimal","value":"1e-130"},
                {"type":"decimal","value":"9.9e125"}
            ]
        }),
    )
    .await;
    common::succeeded(&result);
    assert_eq!(
        result["rows"],
        json!([[
            null,
            "AP8=",
            "-12345678901234567890.12345678",
            "0.000000000001",
            "18446744073709551615",
            "2026-10-31T11:22:33.123456789",
            "2026-10-31T14:22:33.987654321",
            "2026-10-31T00:00:00.000000000",
            2.5,
            "1",
            "O'Reilly; SELECT ignored",
            "0",
            format!("0.{}1", "0".repeat(129)),
            format!("99{}", "0".repeat(124))
        ]])
    );
}

#[tokio::test]
#[ignore = "Dedicated Oracle container; DB_MCP_INTEGRATION=1"]
async fn timestamps_with_time_zones_are_returned_in_utc() {
    let fixture = common::Fixture::network(Engine::Oracle);
    let service = fixture.service();
    let table = format!(
        "dbmcp_tz_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let setup_table = table.clone();
    tokio::task::spawn_blocking(move || {
        let config = oracledb::Config::default()
            .set_credentials("dbmcp_test", "DisposableTest123")
            .set_connect_string("127.0.0.1:51521/FREEPDB1")
            .unwrap();
        let mut connection = oracledb::connect(config).unwrap();
        // The generic SQL parser does not support LOCAL TIME ZONE type declarations.
        connection
            .execute(
                &format!(
                    "CREATE TABLE {setup_table}(zoned_value TIMESTAMP(9) WITH TIME ZONE,
                                       local_value TIMESTAMP(9) WITH LOCAL TIME ZONE)"
                ),
                &[],
            )
            .unwrap();
        connection
            .execute(
                &format!(
                    "INSERT INTO {setup_table}
             SELECT stamp, stamp FROM (
                 SELECT TO_TIMESTAMP_TZ('2026-10-31 11:22:33.123456789 -03:00',
                        'YYYY-MM-DD HH24:MI:SS.FF9 TZH:TZM') AS stamp FROM dual
             )"
                ),
                &[],
            )
            .unwrap();
        connection.commit().unwrap();
        connection.close().unwrap();
    })
    .await
    .unwrap();
    common::wait_for_oracle_ddl(&service, &table).await;
    let inserted = common::request(
        &service,
        json!({
            "database_id":"rw",
            "sql":format!("INSERT INTO {table}(zoned_value,local_value) VALUES(:1,:2)"),
            "parameters":[
                {"type":"timestamp","value":"2026-10-31T11:22:33.123456789-03:00"},
                {"type":"timestamp","value":"2026-10-31T11:22:33.123456789-03:00"}
            ]
        }),
    )
    .await;
    common::succeeded(&inserted);
    let result = common::execute(
        &service,
        "ro",
        &format!("SELECT zoned_value,local_value FROM {table}"),
    )
    .await;
    common::succeeded(&result);
    assert_eq!(
        result["rows"],
        json!([
            [
                "2026-10-31T14:22:33.123456789Z",
                "2026-10-31T14:22:33.123456789Z"
            ],
            [
                "2026-10-31T14:22:33.123456789Z",
                "2026-10-31T14:22:33.123456789Z"
            ]
        ])
    );
    common::succeeded(&common::execute(&service, "ddl", &format!("DROP TABLE {table}")).await);
}

#[tokio::test]
#[ignore = "Dedicated Oracle container; DB_MCP_INTEGRATION=1"]
async fn invalid_late_binding_is_rejected_before_any_batch_write() {
    let fixture = common::Fixture::network(Engine::Oracle);
    let service = fixture.service();
    let table = format!(
        "dbmcp_bind_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    common::succeeded(
        &common::execute(
            &service,
            "ddl",
            &format!("CREATE TABLE {table}(id NUMBER PRIMARY KEY, amount NUMBER)"),
        )
        .await,
    );
    common::wait_for_oracle_ddl(&service, &table).await;

    let rejected = common::request(
        &service,
        json!({
            "database_id": "rw",
            "statements": [
                {"sql":format!("INSERT INTO {table}(id,amount) VALUES(1,1)")},
                {"sql":format!("INSERT INTO {table}(id,amount) VALUES(2,:1)"),
                 "parameters":[{"type":"decimal","value":"1e126"}]}
            ]
        }),
    )
    .await;
    assert_eq!(rejected["error"]["code"], "INVALID_ARGUMENT", "{rejected}");
    assert_eq!(rejected["execution_state"], "not_started");
    assert_eq!(rejected["error"]["failed_statement_index"], 1);
    let observed = common::execute(&service, "ro", &format!("SELECT COUNT(*) FROM {table}")).await;
    common::succeeded(&observed);
    assert_eq!(observed["rows"], json!([["0"]]));
    common::succeeded(&common::execute(&service, "ddl", &format!("DROP TABLE {table}")).await);
}

#[tokio::test]
#[ignore = "Dedicated Oracle container; DB_MCP_INTEGRATION=1"]
async fn read_timeout_returns_without_claiming_write_effects() {
    let fixture = common::Fixture::network(Engine::Oracle);
    let mut catalog = fixture.catalog();
    Arc::make_mut(catalog.databases.get_mut("ro").unwrap())
        .limits
        .query_timeout_seconds = 2;
    let service = ExecutionService::new(catalog);
    let sql =
        "SELECT COUNT(*) FROM all_objects a CROSS JOIN all_objects b CROSS JOIN all_objects c";
    let started = std::time::Instant::now();
    let result = common::execute(&service, "ro", sql).await;
    assert!(started.elapsed() < Duration::from_secs(5), "{result}");
    assert_eq!(result["error"]["code"], "EXECUTION_TIMEOUT", "{result}");
    assert_eq!(result["effect"], "none");
}
