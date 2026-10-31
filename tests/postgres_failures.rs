#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;

use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use db_mcp::{config::Engine, execution::ExecutionService};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[tokio::test]
#[ignore = "Dedicated PostgreSQL container; DB_MCP_INTEGRATION=1"]
async fn lost_commit_response_is_reported_unknown_and_the_write_is_not_repeated() {
    let fixture = common::Fixture::network(Engine::Postgresql);
    let table = format!(
        "dbmcp_fault_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    common::succeeded(
        &common::execute(
            &fixture.service(),
            "ddl",
            &format!("CREATE TABLE {table}(id INTEGER)"),
        )
        .await,
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let proxy = tokio::spawn(async move {
        let (client, _) = listener.accept().await.unwrap();
        let server = TcpStream::connect("127.0.0.1:55432").await.unwrap();
        let (mut cr, mut cw) = client.into_split();
        let (mut sr, mut sw) = server.into_split();
        let commit_sent = Arc::new(AtomicBool::new(false));
        let sent = commit_sent.clone();
        let upstream = async move {
            let mut buffer = [0u8; 8192];
            let mut tail = Vec::new();
            loop {
                let n = cr.read(&mut buffer).await.unwrap();
                if n == 0 {
                    break;
                }
                tail.extend_from_slice(&buffer[..n]);
                if tail.windows(7).any(|s| s == b"COMMIT\0") {
                    sent.store(true, Ordering::SeqCst);
                }
                if tail.len() > 32 {
                    tail.drain(..tail.len() - 32);
                }
                sw.write_all(&buffer[..n]).await.unwrap();
            }
        };
        let downstream = async move {
            let mut buffer = [0u8; 8192];
            let mut tail = Vec::new();
            loop {
                let n = sr.read(&mut buffer).await.unwrap();
                if n == 0 {
                    break;
                }
                tail.extend_from_slice(&buffer[..n]);
                if commit_sent.load(Ordering::SeqCst) && tail.windows(7).any(|s| s == b"COMMIT\0") {
                    // Server confirmed commit to the proxy. Deliberately lose that response.
                    cw.shutdown().await.unwrap();
                    return true;
                }
                if tail.len() > 32 {
                    tail.drain(..tail.len() - 32);
                }
                cw.write_all(&buffer[..n]).await.unwrap();
            }
            false
        };
        tokio::select! { _=upstream => false, committed=downstream => committed }
    });
    let path = fixture.directory.path().join("databases/rw.toml");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        original.replace("127.0.0.1:55432", &format!("127.0.0.1:{port}")),
    )
    .unwrap();
    let service = fixture.service();
    let result = common::execute(
        &service,
        "rw",
        &format!("INSERT INTO {table}(id) VALUES(1)"),
    )
    .await;
    assert!(
        tokio::time::timeout(Duration::from_secs(10), proxy)
            .await
            .unwrap()
            .unwrap()
    );
    assert_eq!(result["error"]["code"], "WRITE_OUTCOME_UNKNOWN", "{result}");
    assert_eq!(result["effect"], "unknown");
    let observed = common::execute(&service, "ro", &format!("SELECT COUNT(*) FROM {table}")).await;
    assert_eq!(
        observed["rows"][0][0], 1,
        "A committed insert must occur exactly once: {observed}"
    );
    common::succeeded(&common::execute(&service, "ddl", &format!("DROP TABLE {table}")).await);
}

#[tokio::test]
#[ignore = "Dedicated PostgreSQL container; DB_MCP_INTEGRATION=1"]
async fn read_timeout_is_bounded_without_claiming_write_effects() {
    let fixture = common::Fixture::network(Engine::Postgresql);
    let mut catalog = fixture.catalog();
    Arc::make_mut(catalog.databases.get_mut("ro").unwrap())
        .limits
        .query_timeout_seconds = 1;
    let service = ExecutionService::new(catalog);
    let started = std::time::Instant::now();
    let result = common::execute(&service, "ro", "SELECT pg_sleep(5)").await;
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_ne!(result["execution_state"], "succeeded");
    // The deadline includes connection setup, so not_started is valid under load.
    assert!(matches!(
        result["execution_state"].as_str(),
        Some("not_started" | "failed")
    ));
    assert_eq!(result["effect"], "none");
}

#[tokio::test]
#[ignore = "Dedicated PostgreSQL container; DB_MCP_INTEGRATION=1"]
async fn cancellation_after_observed_execution_does_not_claim_not_started() {
    use sqlx::Connection;
    use tokio_util::sync::CancellationToken;

    let fixture = common::Fixture::network(Engine::Postgresql);
    let mut catalog = fixture.catalog();
    Arc::make_mut(catalog.databases.get_mut("ro").unwrap())
        .limits
        .query_timeout_seconds = 30;
    let service = ExecutionService::new(catalog);
    let marker = uuid::Uuid::new_v4().simple().to_string();
    let token = CancellationToken::new();
    let cancellation = token.clone();
    let sql = format!("SELECT pg_sleep(60) /* {marker} */");
    let request = serde_json::from_value(serde_json::json!({
        "database_id": "ro",
        "sql": sql
    }))
    .unwrap();
    let options = sqlx::postgres::PgConnectOptions::new()
        .host("127.0.0.1")
        .port(55432)
        .database("dbmcp_test")
        .username("dbmcp_test")
        .password("disposable-test-password");
    let mut observer = sqlx::PgConnection::connect_with(&options).await.unwrap();
    let operation = tokio::spawn(async move { service.execute(request, cancellation).await });

    // Synchronize on the real server query, not a delay guessed from local CPU speed.
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut interval = tokio::time::interval(Duration::from_millis(25));
        loop {
            interval.tick().await;
            let observed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE query LIKE $1 AND state='active' AND wait_event='PgSleep')"
            ).bind(format!("%{marker}%")).fetch_one(&mut observer).await.unwrap();
            if observed { break; }
        }
    }).await.unwrap();
    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["error"]["code"], "EXECUTION_CANCELLED", "{result}");
    assert_eq!(result["execution_state"], "failed");
    assert_eq!(result["effect"], "none");
}
