#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;

use db_mcp::config::Engine;
use serde_json::{Value, json};
use sqlx::{Connection, Executor, SqlSafeStr};

fn context(engine: Engine, package: &str, tenant: &str) -> Value {
    let sql = match engine {
        Engine::Postgresql => "SELECT pg_catalog.set_config('app.tenant_id', $1, true)".into(),
        Engine::Oracle => format!("CALL {package}.set_tenant(:1)"),
        _ => panic!("This fixture tests PostgreSQL and Oracle session context"),
    };
    json!({"sql":sql,"parameters":[{"type":"string","value":tenant}]})
}

async fn context_contract(fixture: &common::Fixture, table: &str, package: &str) {
    let service = fixture.service();
    let query = format!("SELECT status FROM {table} ORDER BY id");
    let initial = common::execute(&service, "ro", &query).await;
    common::succeeded(&initial);
    assert_eq!(initial["rows"], json!([]));

    // The context is visible to both queries and remains usable in read-only mode.
    let read = common::request(
        &service,
        json!({
            "database_id":"ro",
            "statements":[context(fixture.engine, package, "42"), {"sql":query}, {"sql":query}]
        }),
    )
    .await;
    common::succeeded(&read);
    assert_eq!(read["results"].as_array().unwrap().len(), 3);
    assert_eq!(read["results"][1]["rows"], json!([["tenant42"]]));
    assert_eq!(read["results"][2]["rows"], json!([["tenant42"]]));

    let next_call = common::execute(&service, "ro", &query).await;
    common::succeeded(&next_call);
    assert_eq!(next_call["rows"], json!([]));

    let write = common::request(
        &service,
        json!({
            "database_id":"rw",
            "statements":[
                context(fixture.engine, package, "42"),
                {"sql":format!("UPDATE {table} SET status='processed'")},
                {"sql":query}
            ]
        }),
    )
    .await;
    common::succeeded(&write);
    assert_eq!(write["effect"], "committed");
    assert_eq!(write["results"][2]["rows"], json!([["processed"]]));

    let error_sql = match fixture.engine {
        Engine::Oracle => "SELECT 1/0 FROM dual",
        _ => "SELECT 1/0",
    };
    let failed = common::request(
        &service,
        json!({
            "database_id":"rw",
            "statements":[
                context(fixture.engine, package, "42"),
                {"sql":format!("UPDATE {table} SET status='must_rollback'")},
                {"sql":error_sql}
            ]
        }),
    )
    .await;
    assert_eq!(failed["error"]["code"], "SQL_ERROR", "{failed}");
    assert_eq!(failed["error"]["failed_statement_index"], 2);
    let expected_effect = match fixture.engine {
        Engine::Oracle => "unknown", // A package can commit internally; rollback alone is not proof.
        _ => "rolled_back",
    };
    assert_eq!(failed["effect"], expected_effect);

    for (tenant, expected) in [("42", "processed"), ("99", "tenant99")] {
        let persisted = common::request(
            &service,
            json!({"database_id":"ro","statements":[
                context(fixture.engine, package, tenant), {"sql":query}
            ]}),
        )
        .await;
        common::succeeded(&persisted);
        assert_eq!(persisted["results"][1]["rows"], json!([[expected]]));
    }
}

fn increase_row_budget(fixture: &common::Fixture) {
    let config = std::fs::read_to_string(&fixture.global)
        .unwrap()
        .replace("max_rows=2", "max_rows=10");
    std::fs::write(&fixture.global, config).unwrap();
}

#[tokio::test]
#[ignore = "Dedicated PostgreSQL container; DB_MCP_INTEGRATION=1"]
async fn postgres_rls_uses_context_for_the_whole_transaction() {
    let fixture = common::Fixture::network(Engine::Postgresql);
    increase_row_budget(&fixture);
    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let table = format!("dbmcp_ctx_{suffix}");
    let role = format!("dbmcp_ctx_user_{suffix}");
    let mut admin = sqlx::PgConnection::connect(
        "postgresql://dbmcp_test:disposable-test-password@127.0.0.1:55432/dbmcp_test",
    )
    .await
    .unwrap();
    // FORCE RLS also needs a non-superuser: the container's initial user bypasses RLS.
    for sql in [
        format!("CREATE ROLE {role} LOGIN PASSWORD 'disposable-test-password'"),
        format!(
            "CREATE TABLE {table}(id INTEGER PRIMARY KEY, tenant_id INTEGER, status VARCHAR(30))"
        ),
        format!("INSERT INTO {table} VALUES(1,42,'tenant42'),(2,99,'tenant99')"),
        format!("ALTER TABLE {table} OWNER TO {role}"),
        format!("ALTER TABLE {table} ENABLE ROW LEVEL SECURITY"),
        format!("ALTER TABLE {table} FORCE ROW LEVEL SECURITY"),
        format!(
            "CREATE POLICY tenant_filter ON {table}
                 USING (tenant_id = NULLIF(current_setting('app.tenant_id', true), '')::integer)"
        ),
    ] {
        admin
            .execute(sqlx::AssertSqlSafe(sql).into_sql_str())
            .await
            .unwrap();
    }
    for id in ["ro", "rw", "ddl"] {
        let path = fixture
            .directory
            .path()
            .join(format!("databases/{id}.toml"));
        let config = std::fs::read_to_string(&path)
            .unwrap()
            .replace("username=\"dbmcp_test\"", &format!("username=\"{role}\""));
        std::fs::write(path, config).unwrap();
    }

    context_contract(&fixture, &table, "").await;
    let local = common::request(
        &fixture.service(),
        json!({"database_id":"ro","statements":[
            {"sql":"SET LOCAL app.tenant_id = '99'"},
            {"sql":format!("SELECT status FROM {table}")}
        ]}),
    )
    .await;
    common::succeeded(&local);
    assert_eq!(local["results"][1]["rows"], json!([["tenant99"]]));

    admin
        .execute(sqlx::AssertSqlSafe(format!("DROP TABLE {table}")).into_sql_str())
        .await
        .unwrap();
    admin
        .execute(sqlx::AssertSqlSafe(format!("DROP ROLE {role}")).into_sql_str())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "Dedicated Oracle container; DB_MCP_INTEGRATION=1"]
async fn oracle_vpd_uses_package_context_and_preserves_uncertain_routine_effects() {
    let fixture = common::Fixture::network(Engine::Oracle);
    increase_row_budget(&fixture);
    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let table = format!("dbmcp_ctx_{suffix}");
    let package = format!("dbmcp_pkg_{suffix}");
    let namespace = format!("dbmcp_ns_{suffix}");
    let setup_table = table.clone();
    let setup_package = package.clone();
    let setup_namespace = namespace.clone();
    tokio::task::spawn_blocking(move || {
        let config = oracledb::Config::default()
            .set_credentials("system", "DisposableRoot123")
            .set_connect_string("127.0.0.1:51521/FREEPDB1").unwrap();
        let connection = oracledb::connect(config).unwrap();
        for sql in [
            format!("CREATE TABLE dbmcp_test.{setup_table}(id INTEGER PRIMARY KEY, tenant_id INTEGER, status VARCHAR2(30))"),
            format!("INSERT INTO dbmcp_test.{setup_table} VALUES(1,42,'tenant42')"),
            format!("INSERT INTO dbmcp_test.{setup_table} VALUES(2,99,'tenant99')"),
            format!("CREATE PACKAGE dbmcp_test.{setup_package} AS
                PROCEDURE set_tenant(tenant VARCHAR2);
                PROCEDURE write_status;
                PROCEDURE commit_status;
                FUNCTION predicate(owner VARCHAR2, object_name VARCHAR2) RETURN VARCHAR2;
                END;"),
            format!("CREATE PACKAGE BODY dbmcp_test.{setup_package} AS
                PROCEDURE set_tenant(tenant VARCHAR2) IS BEGIN
                    IF tenant NOT IN ('42','99') THEN RAISE_APPLICATION_ERROR(-20001,'Invalid tenant'); END IF;
                    DBMS_SESSION.SET_CONTEXT('{setup_namespace}', 'TENANT_ID', tenant);
                END;
                PROCEDURE write_status IS BEGIN
                    set_tenant('42');
                    UPDATE {setup_table} SET status='committed_inside' WHERE id=1;
                END;
                PROCEDURE commit_status IS BEGIN write_status; COMMIT; END;
                FUNCTION predicate(owner VARCHAR2, object_name VARCHAR2) RETURN VARCHAR2 IS BEGIN
                    RETURN 'tenant_id = TO_NUMBER(SYS_CONTEXT(''{setup_namespace}'',''TENANT_ID''))';
                END;
                END;"),
            format!("CREATE CONTEXT {setup_namespace} USING dbmcp_test.{setup_package}"),
            format!("BEGIN DBMS_RLS.ADD_POLICY(object_schema=>'DBMCP_TEST',object_name=>'{}',
                policy_name=>'tenant_filter',function_schema=>'DBMCP_TEST',
                policy_function=>'{setup_package}.predicate',statement_types=>'SELECT,UPDATE,DELETE',
                policy_type=>DBMS_RLS.DYNAMIC); END;", setup_table.to_uppercase()),
        ] {
            connection.execute(&sql, &[]).unwrap();
        }
        connection.commit().unwrap();
    }).await.unwrap();
    let service = fixture.service();
    common::wait_for_oracle_ddl(&service, &table).await;
    context_contract(&fixture, &table, &package).await;

    // A failed setup stops before the following write.
    let invalid = common::request(
        &service,
        json!({"database_id":"rw","statements":[
            context(Engine::Oracle, &package, "invalid"),
            {"sql":format!("UPDATE {table} SET status='must_not_run'")}
        ]}),
    )
    .await;
    assert_eq!(invalid["error"]["database_code"], "ORA-20001", "{invalid}");
    assert_eq!(invalid["error"]["failed_statement_index"], 0);

    // Hidden DML is still rejected by Oracle's native read-only transaction.
    let denied = common::execute(&service, "ro", &format!("CALL {package}.write_status()")).await;
    assert_eq!(denied["error"]["database_code"], "ORA-01456", "{denied}");
    let check = common::request(
        &service,
        json!({"database_id":"ro","statements":[
            context(Engine::Oracle, &package, "42"), {"sql":format!("SELECT status FROM {table}")}
        ]}),
    )
    .await;
    common::succeeded(&check);
    assert_eq!(check["results"][1]["rows"], json!([["processed"]]));

    // The server cannot undo a commit performed inside a stored package.
    let committed_inside = common::request(
        &service,
        json!({"database_id":"rw","statements":[
            {"sql":format!("CALL {package}.commit_status()")}, {"sql":"SELECT 1/0 FROM dual"}
        ]}),
    )
    .await;
    assert_eq!(committed_inside["effect"], "unknown", "{committed_inside}");
    let persisted = common::request(
        &service,
        json!({"database_id":"ro","statements":[
            context(Engine::Oracle, &package, "42"), {"sql":format!("SELECT status FROM {table}")}
        ]}),
    )
    .await;
    common::succeeded(&persisted);
    assert_eq!(
        persisted["results"][1]["rows"],
        json!([["committed_inside"]])
    );

    tokio::task::spawn_blocking(move || {
        let config = oracledb::Config::default()
            .set_credentials("system", "DisposableRoot123")
            .set_connect_string("127.0.0.1:51521/FREEPDB1")
            .unwrap();
        let connection = oracledb::connect(config).unwrap();
        for sql in [
            format!("DROP CONTEXT {namespace}"),
            format!("DROP TABLE dbmcp_test.{table} PURGE"),
            format!("DROP PACKAGE dbmcp_test.{package}"),
        ] {
            connection.execute(&sql, &[]).unwrap();
        }
    })
    .await
    .unwrap();
}
