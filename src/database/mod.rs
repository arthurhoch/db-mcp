use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::OwnedSemaphorePermit;

use crate::{
    config::{DatabaseConfig, Engine},
    error::{Effect, Result},
    model::StatementResult,
    policy::PreparedStatement,
};

pub mod metadata;
mod oracle;
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
    }
}

pub fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
