#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::{fs, path::PathBuf};

use db_mcp::{
    config::{Catalog, Engine},
    execution::ExecutionService,
    model::ExecuteRequest,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

pub struct Fixture {
    pub directory: TempDir,
    pub global: PathBuf,
    pub engine: Engine,
}

impl Fixture {
    pub fn sqlite() -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("sample.db"), []).unwrap();
        Self::create(directory, Engine::Sqlite, "sqlite:../sample.db", "", "")
    }

    pub fn network(engine: Engine) -> Self {
        assert_eq!(
            std::env::var("DB_MCP_INTEGRATION").as_deref(),
            Ok("1"),
            "Use somente os containers descartáveis de tests/compose.yaml e defina DB_MCP_INTEGRATION=1."
        );
        let (url, password) = match engine {
            Engine::Postgresql => (
                "postgresql://127.0.0.1:55432/dbmcp_test",
                "disposable-test-password",
            ),
            Engine::Mysql => (
                "mysql://127.0.0.1:53306/dbmcp_test",
                "disposable-test-password",
            ),
            Engine::Oracle => ("oracle://127.0.0.1:51521/FREEPDB1", "DisposableTest123"),
            Engine::Sqlite => panic!("Use Fixture::sqlite"),
        };
        Self::create(
            tempfile::tempdir().unwrap(),
            engine,
            url,
            "dbmcp_test",
            password,
        )
    }

    fn create(
        directory: TempDir,
        engine: Engine,
        url: &str,
        username: &str,
        password: &str,
    ) -> Self {
        fs::create_dir(directory.path().join("databases")).unwrap();
        let global = directory.path().join("global.toml");
        fs::write(&global, "config_version=1\ndatabases_dir='databases'\n[server]\ntransport='stdio'\n[execution]\nmax_rows=2\nmax_result_bytes=16384\nquery_timeout_seconds=5\n").unwrap();
        if engine == Engine::Oracle {
            let content = fs::read_to_string(&global)
                .unwrap()
                .replace("query_timeout_seconds=5", "query_timeout_seconds=30");
            fs::write(&global, content).unwrap();
        }
        for (id, mode) in [
            ("ro", "read_only"),
            ("rw", "read_write"),
            ("ddl", "read_write_ddl"),
        ] {
            let credentials = if engine == Engine::Sqlite {
                String::new()
            } else {
                format!("username={username:?}\npassword={password:?}\n")
            };
            let config = format!(
                "config_version=1\nid={id:?}\nname='Test connection'\naliases=['ambiguous alias']\ndescription='Dedicated disposable test database'\nengine={:?}\naccess_mode={mode:?}\n[connection]\nurl={url:?}\n{credentials}",
                engine.as_str()
            );
            fs::write(
                directory.path().join(format!("databases/{id}.toml")),
                config,
            )
            .unwrap();
        }
        Self {
            directory,
            global,
            engine,
        }
    }

    pub fn catalog(&self) -> Catalog {
        Catalog::load(&self.global).unwrap()
    }
    pub fn service(&self) -> ExecutionService {
        ExecutionService::new(self.catalog())
    }
    pub fn placeholder(&self, index: usize) -> String {
        match self.engine {
            Engine::Postgresql => format!("${index}"),
            Engine::Oracle => format!(":{index}"),
            Engine::Mysql => "?".into(),
            Engine::Sqlite => format!("?{index}"),
        }
    }
}

pub async fn execute(service: &ExecutionService, id: &str, sql: &str) -> Value {
    request(service, json!({"database_id":id,"sql":sql})).await
}

pub async fn request(service: &ExecutionService, input: Value) -> Value {
    let request: ExecuteRequest = serde_json::from_value(input).unwrap();
    service.execute(request, CancellationToken::new()).await
}

pub fn succeeded(result: &Value) {
    assert_eq!(result["execution_state"], "succeeded", "{result}");
}

pub async fn wait_for_oracle_ddl(service: &ExecutionService, table: &str) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
        interval.tick().await;
        let result = execute(
            service,
            "ro",
            &format!(
                "SELECT CASE WHEN last_ddl_time < SYSDATE-(2/86400) THEN 1 ELSE 0 END
             FROM user_objects WHERE object_name='{}'",
                table.to_uppercase()
            ),
        )
        .await;
        succeeded(&result);
        let value = &result["rows"][0][0];
        if value == 1 || value == "1" {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Oracle DDL timestamp did not advance"
        );
    }
}
