#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;

use std::{ffi::OsString, process::Stdio, time::Duration};

use db_mcp::config::Engine;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout, Command},
};

async fn send(input: &mut ChildStdin, value: Value) {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    input.write_all(&bytes).await.unwrap();
}

async fn receive(output: &mut BufReader<ChildStdout>, id: u64) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut line = String::new();
            assert!(
                output.read_line(&mut line).await.unwrap() > 0,
                "MCP process closed stdout"
            );
            let message: Value =
                serde_json::from_str(&line).expect("stdout must contain JSON-RPC only");
            if message["id"] == id {
                return message;
            }
        }
    })
    .await
    .unwrap()
}

async fn initialize(input: &mut ChildStdin, output: &mut BufReader<ChildStdout>) {
    send(
        input,
        json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize",
            "params": {
                "protocolVersion":"2025-11-25",
                "capabilities":{},
                "clientInfo":{"name":"db-mcp-test","version":"1"}
            }
        }),
    )
    .await;
    let initialized = receive(output, 1).await;
    assert_eq!(initialized["result"]["serverInfo"]["name"], "db-mcp");
    send(
        input,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
}

fn server_binary() -> OsString {
    std::env::var_os("DB_MCP_TEST_BINARY").unwrap_or_else(|| env!("CARGO_BIN_EXE_db-mcp").into())
}

async fn stdio_contract(fixture: common::Fixture) {
    let mut child = Command::new(server_binary())
        .arg("--config")
        .arg(&fixture.global)
        .env_remove("DYLD_LIBRARY_PATH")
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("ORACLE_HOME")
        .env_remove("RSO_DEBUG_PACKETS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    initialize(&mut input, &mut output).await;
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    )
    .await;
    let tools = receive(&mut output, 2).await;
    let tools = tools["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 3);
    let execute = tools
        .iter()
        .find(|tool| tool["name"] == "execute_sql")
        .unwrap();
    assert_eq!(execute["annotations"]["readOnlyHint"], false);
    assert_eq!(execute["annotations"]["destructiveHint"], true);
    send(&mut input,json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_databases","arguments":{}}})).await;
    let catalog = receive(&mut output, 3).await;
    assert!(catalog["result"]["structuredContent"]["databases"].is_array());
    assert!(!catalog.to_string().contains("sqlite:"));
    assert!(!catalog.to_string().contains("disposable-test-password"));
    send(
        &mut input,
        json!({
            "jsonrpc":"2.0", "id":4, "method":"tools/call",
            "params": {
                "name":"describe_database",
                "arguments":{"database_id":"ro","include":"summary"}
            }
        }),
    )
    .await;
    let described = receive(&mut output, 4).await;
    assert_eq!(
        described["result"]["structuredContent"]["engine"],
        fixture.engine.as_str(),
        "{described}"
    );
    assert!(
        described["result"]["structuredContent"]["version"]
            .as_str()
            .is_some_and(|version| !version.is_empty())
    );

    let sql = match fixture.engine {
        Engine::Oracle => "SELECT :1 AS answer FROM dual",
        Engine::Postgresql => "SELECT $1 AS answer",
        Engine::Mysql | Engine::Sqlite => "SELECT ? AS answer",
    };
    send(
        &mut input,
        json!({
            "jsonrpc":"2.0", "id":5, "method":"tools/call",
            "params": {
                "name":"execute_sql",
                "arguments": {
                    "database_id":"ro", "sql":sql,
                    "parameters":[{"type":"integer","value":"42"}]
                }
            }
        }),
    )
    .await;
    let result = receive(&mut output, 5).await;
    let expected = if fixture.engine == Engine::Oracle {
        json!([["42"]])
    } else {
        json!([[42]])
    };
    assert_eq!(
        result["result"]["structuredContent"]["rows"], expected,
        "{result}"
    );
    send(&mut input,json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"execute_sql","arguments":{"database_id":"ro","sql":"DELETE FROM protected_data"}}})).await;
    let denied = receive(&mut output, 6).await;
    assert_eq!(denied["result"]["isError"], true);
    assert_eq!(
        denied["result"]["structuredContent"]["error"]["code"],
        "READ_ONLY_VIOLATION"
    );
    drop(input);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn stdio_negotiates_tools_and_executes_sql_without_protocol_log_pollution() {
    stdio_contract(common::Fixture::sqlite()).await;
}

#[tokio::test]
#[ignore = "Dedicated PostgreSQL container; DB_MCP_INTEGRATION=1"]
async fn stdio_executes_postgres_bindings() {
    stdio_contract(common::Fixture::network(Engine::Postgresql)).await;
}

#[tokio::test]
#[ignore = "Dedicated MySQL container; DB_MCP_INTEGRATION=1"]
async fn stdio_executes_mysql_bindings() {
    stdio_contract(common::Fixture::network(Engine::Mysql)).await;
}

#[tokio::test]
#[ignore = "Dedicated Oracle container; DB_MCP_INTEGRATION=1"]
async fn stdio_executes_oracle_bindings_without_client_libraries() {
    stdio_contract(common::Fixture::network(Engine::Oracle)).await;
}

#[tokio::test]
async fn oracle_packet_tracing_is_rejected_before_it_can_pollute_stdout() {
    let fixture = common::Fixture::sqlite();
    std::fs::write(
        fixture.directory.path().join("databases/ro.toml"),
        r#"
config_version=1
id="ro"
name="Oracle tracing test"
description="Unused connection: tracing must be rejected before connect"
engine="oracle"
access_mode="read_only"
[connection]
url="oracle://127.0.0.1:1/unused"
username="test"
password="test"
"#,
    )
    .unwrap();
    let mut child = Command::new(server_binary())
        .arg("--config")
        .arg(&fixture.global)
        .env("RSO_DEBUG_PACKETS", "private-sentinel")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    initialize(&mut input, &mut output).await;
    send(
        &mut input,
        json!({
            "jsonrpc":"2.0", "id":2, "method":"tools/call",
            "params": {
                "name":"execute_sql",
                "arguments":{"database_id":"ro","sql":"SELECT 1 FROM dual"}
            }
        }),
    )
    .await;
    let response = receive(&mut output, 2).await;
    let result = &response["result"]["structuredContent"];
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert_eq!(result["error"]["code"], "CONFIG_INVALID");
    assert_eq!(result["execution_state"], "not_started");
    assert!(!response.to_string().contains("private-sentinel"));
    drop(input);
    assert!(
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    let mut diagnostics = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut diagnostics)
        .await
        .unwrap();
    assert!(!diagnostics.contains("private-sentinel"));
}
