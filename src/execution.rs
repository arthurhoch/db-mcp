use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::{
    config::{Catalog, DatabaseConfig},
    database::{self, Job, Outcome, Progress},
    error::{AppError, Effect, ErrorCode, ExecutionState, Result},
    model::{DescribeRequest, ExecuteRequest, ListRequest, MetadataKind},
    policy::{self, PreparedStatement, StatementKind},
};

pub struct ExecutionService {
    catalog: Catalog,
    operations: Arc<Semaphore>,
    connections: Arc<Semaphore>,
    per_database: BTreeMap<String, Arc<Semaphore>>,
}

impl ExecutionService {
    pub fn new(catalog: Catalog) -> Self {
        Self {
            operations: Arc::new(Semaphore::new(
                catalog.global.execution.max_concurrent_operations,
            )),
            connections: Arc::new(Semaphore::new(catalog.global.connections.max_open_total)),
            per_database: catalog
                .databases
                .keys()
                .map(|id| {
                    (
                        id.clone(),
                        Arc::new(Semaphore::new(
                            catalog.global.connections.max_open_per_database,
                        )),
                    )
                })
                .collect(),
            catalog,
        }
    }

    pub fn list(&self, request: ListRequest) -> Result<Value> {
        let offset = request.cursor.map_or(Ok(0), |cursor| {
            cursor
                .parse::<usize>()
                .map_err(|_| AppError::invalid("Cursor inválido."))
        })?;
        if offset > self.catalog.databases.len() {
            return Err(AppError::invalid("Cursor fora do catálogo."));
        }
        let mut databases = Vec::new();
        let max_bytes = self.catalog.global.execution.max_result_bytes;
        let mut next = offset;
        for database in self.catalog.databases.values().skip(offset) {
            let entry = json!({
                "id": database.id,
                "name": database.name,
                "aliases": database.aliases,
                "description": database.description,
                "engine": database.engine,
                "access_mode": database.access_mode
            });
            databases.push(entry);
            let prospective =
                json!({"databases":databases,"truncated":true,"next_cursor":(next+1).to_string()});
            if serde_json::to_vec(&prospective)
                .map_err(|_| {
                    AppError::new(ErrorCode::SerializationError, "Catálogo não representável.")
                })?
                .len()
                > max_bytes
                || databases.len() > self.catalog.global.execution.max_rows
            {
                databases.pop();
                if databases.is_empty() {
                    return Err(AppError::new(
                        ErrorCode::LimitExceeded,
                        "Uma entrada do catálogo excede o orçamento; reduza a descrição ou amplie o limite.",
                    ));
                }
                break;
            }
            next += 1;
        }
        let truncated = next < self.catalog.databases.len();
        Ok(
            json!({"databases":databases,"truncated":truncated,"next_cursor":if truncated {Some(next.to_string())} else {None}}),
        )
    }

    pub async fn execute(&self, request: ExecuteRequest, cancellation: CancellationToken) -> Value {
        let id = request.database_id.clone();
        let request_id = uuid::Uuid::new_v4().to_string();
        let started = Instant::now();
        let config = match self.catalog.database(&id) {
            Ok(config) => config,
            Err(error) => return error_report(&request_id, &id, error, started),
        };
        let result = request
            .into_statements(&config.limits)
            .and_then(|(statements, batch)| {
                policy::prepare(&config, statements, batch).map(|plans| (plans, batch))
            });
        let (plans, batch) = match result {
            Ok(prepared) => prepared,
            Err(error) => return error_report(&request_id, &id, error, started),
        };
        let outcome = self.run(config.clone(), plans, batch, cancellation).await;
        let mut report = match outcome {
            Ok(outcome) => outcome_report(&request_id, &config, outcome, batch, started),
            Err(error) => error_report(&request_id, &id, error, started),
        };
        if serde_json::to_vec(&report)
            .map_or(true, |bytes| bytes.len() > config.limits.max_result_bytes)
        {
            let effect = match report["effect"].as_str() {
                Some("committed") => Effect::Committed,
                Some("rolled_back") => Effect::RolledBack,
                Some("none") => Effect::None,
                _ => Effect::Unknown,
            };
            let error = AppError::new(
                ErrorCode::SerializationError,
                "Envelope excede o orçamento de saída; os efeitos conhecidos foram preservados.",
            )
            .with_effect(effect);
            report = error_report(&request_id, &id, error, started);
        }
        tracing::info!(
            request_id,
            database_id = %id,
            elapsed_ms = started.elapsed().as_millis() as u64,
            execution_state = report["execution_state"].as_str(),
            effect = report["effect"].as_str(),
            "database_operation"
        );
        report
    }

    pub async fn describe(
        &self,
        request: DescribeRequest,
        cancellation: CancellationToken,
    ) -> Result<Value> {
        let config = self.catalog.database(&request.database_id)?;
        let statement = database::metadata::statement(&config, &request)?;
        let outcome = self
            .run(config.clone(), vec![statement], false, cancellation)
            .await?;
        let result =
            outcome.results.into_iter().next().ok_or_else(|| {
                AppError::new(ErrorCode::SerializationError, "Metadados ausentes.")
            })?;
        let response = if request.include == MetadataKind::Summary {
            let row = result.rows.first().ok_or_else(|| {
                AppError::new(
                    ErrorCode::DatabasePermissionDenied,
                    "A versão do banco não pôde ser consultada.",
                )
            })?;
            json!({
                "database_id": config.id,
                "engine": config.engine,
                "access_mode": config.access_mode,
                "version": row.first(),
                "default_schema": row.get(1),
                "capabilities": {
                    "single_statement": true,
                    "transactional_batch": true,
                    "ddl_mode": "read_write_ddl",
                    "ddl_in_batch": false,
                    "transaction_across_calls": false,
                    "native_placeholders": true,
                    "untrusted_database_content": true
                }
            })
        } else {
            json!({
                "database_id": config.id,
                "engine": config.engine,
                "access_mode": config.access_mode,
                "columns": result.columns,
                "rows": result.rows,
                "returned_rows": result.returned_rows,
                "truncated": result.truncated,
                "limit_reason": result.limit_reason
            })
        };
        if serde_json::to_vec(&response).map_or(true, |v| v.len() > config.limits.max_result_bytes)
        {
            return Err(AppError::new(
                ErrorCode::LimitExceeded,
                "Metadados excedem o orçamento de saída.",
            ));
        }
        Ok(response)
    }

    async fn run(
        &self,
        config: Arc<DatabaseConfig>,
        statements: Vec<PreparedStatement>,
        batch: bool,
        cancellation: CancellationToken,
    ) -> Result<Outcome> {
        let admission = async {
            let per_database = self
                .per_database
                .get(&config.id)
                .cloned()
                .ok_or_else(|| AppError::invalid("Banco sem controle de admissão."))?;
            let mut permits = Vec::new();
            // Acquire per-database first, so one busy destination cannot consume every global slot.
            for semaphore in [
                per_database,
                self.operations.clone(),
                self.connections.clone(),
            ] {
                permits.push(semaphore.acquire_owned().await.map_err(|_| {
                    AppError::new(ErrorCode::ExecutionCancelled, "Servidor encerrando.")
                })?);
            }
            Ok::<Vec<OwnedSemaphorePermit>, AppError>(permits)
        };
        let permits = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(AppError::new(ErrorCode::ExecutionCancelled, "Chamada cancelada antes da execução.")),
            result = tokio::time::timeout(Duration::from_secs(self.catalog.global.connections.acquire_timeout_seconds), admission) => result.map_err(|_| AppError::new(ErrorCode::LimitExceeded, "Tempo de aquisição de conexão excedido."))??,
        };
        let writes = statements.iter().any(|s| s.kind != StatementKind::Read);
        let progress = Arc::new(Progress::default());
        let timeout = Duration::from_secs(config.limits.query_timeout_seconds);
        let job = Job {
            config,
            statements,
            batch,
            progress: progress.clone(),
            permits,
        };
        let outcome = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(interrupted(&progress, writes, ErrorCode::ExecutionCancelled)),
            result = tokio::time::timeout(timeout, database::run(job)) => match result { Ok(result) => result, Err(_) => Err(interrupted(&progress, writes, ErrorCode::ExecutionTimeout)) },
        };
        outcome
    }
}

fn interrupted(progress: &Progress, writes: bool, reason: ErrorCode) -> AppError {
    progress.cancelled.store(true, Ordering::SeqCst);
    if progress.committed.load(Ordering::SeqCst) {
        return AppError::new(
            reason,
            "Resposta interrompida depois de confirmação do commit.",
        )
        .with_effect(Effect::Committed);
    }
    if writes && progress.sent.load(Ordering::SeqCst) {
        return AppError::new(ErrorCode::WriteOutcomeUnknown, "Operação de escrita interrompida após envio; resultado desconhecido, sem retry automático.").with_effect(Effect::Unknown);
    }
    let error = AppError::new(
        reason,
        "Operação interrompida sem confirmação de resultado.",
    );
    if progress.sent.load(Ordering::SeqCst) {
        error.with_effect(Effect::None)
    } else {
        error
    }
}

fn outcome_report(
    request_id: &str,
    config: &DatabaseConfig,
    outcome: Outcome,
    batch: bool,
    started: Instant,
) -> Value {
    let mut report = json!({
        "request_id": request_id,
        "database_id": config.id,
        "engine": config.engine,
        "execution_state": ExecutionState::Succeeded,
        "effect": outcome.effect,
        "elapsed_ms": started.elapsed().as_millis() as u64
    });
    if batch {
        report["results"] = json!(outcome.results);
    } else if let Some(result) = outcome.results.into_iter().next()
        && let (Some(target), Ok(Value::Object(fields))) =
            (report.as_object_mut(), serde_json::to_value(result))
    {
        target.extend(fields);
    }
    report
}

pub fn error_report(
    request_id: &str,
    database_id: &str,
    error: AppError,
    started: Instant,
) -> Value {
    json!({
        "request_id": request_id,
        "database_id": database_id,
        "execution_state": error.execution_state,
        "effect": error.effect,
        "error": error,
        "elapsed_ms": started.elapsed().as_millis() as u64
    })
}
