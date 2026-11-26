use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::sync::OwnedSemaphorePermit;

use crate::{
    config::{DatabaseConfig, Engine},
    error::{Effect, Result},
    model::StatementResult,
    policy::{PreparedStatement, StatementKind},
};

pub mod metadata;
mod oracle;
mod sql_server;
mod sqlx;

#[derive(Default)]
pub struct Progress {
    pub sent: AtomicBool,
    pub committed: AtomicBool,
    pub cancelled: AtomicBool,
}

impl Progress {
    pub fn mark_sent(&self) {
        self.sent.store(true, Ordering::SeqCst);
    }
}

pub struct Job {
    pub config: Arc<DatabaseConfig>,
    pub statements: Vec<PreparedStatement>,
    pub batch: bool,
    pub explain: bool,
    pub progress: Arc<Progress>,
    pub permits: Vec<OwnedSemaphorePermit>,
}

pub struct Outcome {
    pub results: Vec<StatementResult>,
    pub effect: Effect,
}

pub async fn run(job: Job) -> Result<Outcome> {
    match job.config.engine {
        Engine::Postgresql => sqlx::postgres::run(job).await,
        Engine::Mysql => sqlx::mysql::run(job).await,
        Engine::Sqlite => sqlx::sqlite::run(job).await,
        Engine::Oracle => oracle::run(job).await,
        Engine::SqlServer => sql_server::run(job).await,
    }
}

pub fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

// Preflight locks last for the transaction. Check each distinct spelling once,
// in first-use order, without changing the order of the statements themselves.
fn batch_write_relations(statements: &[PreparedStatement]) -> impl Iterator<Item = &str> {
    let mut seen = BTreeSet::new();
    statements
        .iter()
        .filter(|statement| statement.kind == StatementKind::Write)
        .flat_map(|statement| &statement.relations)
        .map(String::as_str)
        .filter(move |relation| seen.insert(*relation))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_visits_each_write_relation_once_in_first_use_order() {
        let statement = |kind, relations: &[&str]| PreparedStatement {
            sql: String::new(),
            parameters: vec![],
            kind,
            relations: relations.iter().map(|name| name.to_string()).collect(),
        };
        let batch = [
            statement(StatementKind::Read, &["audit"]),
            statement(StatementKind::Write, &["orders"]),
            statement(StatementKind::Routine, &["tenant_context"]),
            statement(StatementKind::Write, &["items", "orders"]),
            statement(StatementKind::Write, &["other.orders"]),
        ];
        assert_eq!(
            batch_write_relations(&batch).collect::<Vec<_>>(),
            ["orders", "items", "other.orders"]
        );
    }
}
