#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{env, fs, path::PathBuf};

use db_mcp::{
    config::Catalog,
    execution::ExecutionService,
    model::{DescribeRequest, ExecuteRequest, ExplainRequest},
};
use serde_json::{Value, json};
use tiberius::{AuthMethod, Client, Config, EncryptionLevel};
use tokio::{net::TcpStream, process::Command};
use tokio_util::{compat::TokioAsyncWriteCompatExt, sync::CancellationToken};

async fn execute(service: &ExecutionService, request: Value) -> Value {
    service
        .execute(
            serde_json::from_value::<ExecuteRequest>(request).unwrap(),
            CancellationToken::new(),
        )
        .await
}

#[tokio::test]
#[ignore = "Requires the disposable SQL Server from tests/compose-sql-server.yaml"]
async fn sql_server_contract() {
    let ca_path = PathBuf::from(env::var_os("DB_MCP_SQL_SERVER_TLS_DIR").unwrap()).join("ca.crt");
    let admin_password = env::var("DB_MCP_SQL_SERVER_SA_PASSWORD").unwrap();
    let mut options = Config::new();
    options.host("localhost");
    options.port(51433);
    options.database("master");
    options.encryption(EncryptionLevel::Required);
    options.trust_cert_ca_bundle(fs::read(&ca_path).unwrap());
    options.authentication(AuthMethod::sql_server("sa", admin_password));
    let stream = TcpStream::connect(("127.0.0.1", 51433)).await.unwrap();
    let mut admin = Client::connect(options, stream.compat_write())
        .await
        .unwrap();

    // This fixture can only connect to the test port on loopback.
    admin
        .simple_query("CREATE DATABASE dbmcp_test")
        .await
        .unwrap()
        .into_results()
        .await
        .unwrap();
    let password = format!("DbMcp-{}!", uuid::Uuid::new_v4().simple());
    admin
        .simple_query(format!(
            "CREATE LOGIN dbmcp_reader WITH PASSWORD = N'{password}', DEFAULT_DATABASE = dbmcp_test"
        ))
        .await
        .unwrap()
        .into_results()
        .await
        .unwrap();
    admin
        .simple_query(
            "USE dbmcp_test;
         CREATE USER dbmcp_reader FOR LOGIN dbmcp_reader;
         CREATE TABLE dbo.parent (id int PRIMARY KEY);
         CREATE TABLE dbo.child (
            id int PRIMARY KEY, parent_id int NOT NULL,
            CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES dbo.parent(id)
         );
         CREATE INDEX ix_child_parent ON dbo.child (parent_id);
         INSERT INTO dbo.parent VALUES (1);
         INSERT INTO dbo.child VALUES (42, 1);
         GRANT SELECT ON SCHEMA::dbo TO dbmcp_reader;
         GRANT SHOWPLAN TO dbmcp_reader;",
        )
        .await
        .unwrap()
        .into_results()
        .await
        .unwrap();

    let directory = tempfile::tempdir().unwrap();
    let global = directory.path().join("global.toml");
    let databases = directory.path().join("databases");
    fs::create_dir(&databases).unwrap();
    fs::write(
        &global,
        "config_version=1\ndatabases_dir='databases'\n[server]\ntransport='stdio'\n",
    )
    .unwrap();
    for (id, trust) in [("ro", true), ("untrusted", false)] {
        let mut config = format!(
            "config_version=1\nid={id:?}\nname='SQL Server fixture'\ndescription='Disposable fixture'\nengine='sql_server'\naccess_mode='read_only'\n[connection]\nurl='sqlserver://localhost:51433/dbmcp_test'\nusername='dbmcp_reader'\npassword={password:?}\n"
        );
        if trust {
            use std::fmt::Write as _;
            writeln!(config, "tls_ca_file={ca_path:?}").unwrap();
        }
        fs::write(databases.join(format!("{id}.toml")), config).unwrap();
    }
    let service = ExecutionService::new(Catalog::load(&global).unwrap());

    let sql = "SELECT [id] FROM [dbo].[child] WHERE [id] = @P1";
    let parameters = json!([{"type":"integer","value":"42"}]);
    let result = execute(
        &service,
        json!({"database_id":"ro","sql":sql,"parameters":parameters}),
    )
    .await;
    assert_eq!(result["rows"], json!([[42]]), "{result}");
    let empty = execute(
        &service,
        json!({"database_id":"ro","sql":"SELECT id FROM dbo.child WHERE 1 = 0"}),
    )
    .await;
    assert_eq!(empty["columns"][0]["name"], "id", "{empty}");
    assert_eq!(empty["returned_rows"], 0);
    let decimal = execute(
        &service,
        json!({
            "database_id":"ro","sql":"SELECT @P1 AS n",
            "parameters":[{"type":"decimal","value":"-0.125"}]
        }),
    )
    .await;
    assert_eq!(decimal["rows"], json!([["-0.125"]]), "{decimal}");
    let too_precise = execute(
        &service,
        json!({"database_id":"ro","sql":"SELECT @P1 AS n",
               "parameters":[{"type":"decimal","value":"1e-38"}]}),
    )
    .await;
    assert_eq!(too_precise["error"]["code"], "INVALID_ARGUMENT");
    assert_eq!(too_precise["execution_state"], "not_started");

    let batch = execute(
        &service,
        json!({"database_id":"ro","statements":[
            {"sql":"SELECT id FROM dbo.parent"}, {"sql":"SELECT id FROM dbo.child"}
        ]}),
    )
    .await;
    assert_eq!(batch["results"][0]["rows"], json!([[1]]), "{batch}");
    assert_eq!(batch["results"][1]["rows"], json!([[42]]), "{batch}");

    for (include, expected) in [("indexes", "ix_child_parent"), ("constraints", "fk_parent")] {
        let request: DescribeRequest = serde_json::from_value(json!({
            "database_id":"ro", "include":include, "schema":"dbo", "table":"child"
        }))
        .unwrap();
        let metadata = service
            .describe(request, CancellationToken::new())
            .await
            .unwrap();
        assert!(
            metadata["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row[0] == expected),
            "{metadata}"
        );
    }
    let plan = service
        .explain(
            serde_json::from_value::<ExplainRequest>(json!({
                "database_id":"ro","sql":sql,"parameters":parameters
            }))
            .unwrap(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(plan["plan_type"], "estimated", "{plan}");
    assert!(plan["rows"][0][0].as_str().unwrap().contains("ShowPlanXML"));
    assert_eq!(plan["effect"], "none");

    let untrusted = execute(
        &service,
        json!({"database_id":"untrusted","sql":"SELECT 1"}),
    )
    .await;
    assert_eq!(
        untrusted["error"]["code"], "CONNECTION_FAILED",
        "{untrusted}"
    );
    for sql in [
        "DELETE FROM dbo.child",
        "SELECT id INTO dbo.copy FROM dbo.child",
        "SELECT * FROM master.sys.tables",
        "SELECT * FROM OPENQUERY(remote_server, 'SELECT 1')",
        "SELECT * FROM dbo.child WITH (UPDLOCK)",
        "SELECT NEXT VALUE FOR dbo.sequence_name",
        "SELECT 1; SELECT 2",
    ] {
        let result = execute(&service, json!({"database_id":"ro","sql":sql})).await;
        assert_eq!(result["execution_state"], "not_started", "{sql}: {result}");
        assert!(result.get("error").is_some(), "{sql}: {result}");
    }

    let tested = Command::new(env!("CARGO_BIN_EXE_db-mcp"))
        .arg("--config")
        .arg(&global)
        .args(["--test-connection", "ro"])
        .output()
        .await
        .unwrap();
    assert!(
        tested.status.success(),
        "{}",
        String::from_utf8_lossy(&tested.stderr)
    );
    assert!(String::from_utf8_lossy(&tested.stdout).contains("ro"));
}
