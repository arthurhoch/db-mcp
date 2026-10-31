use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use chrono::{Datelike, NaiveDateTime, NaiveTime, Timelike};
use oracledb::{Connection, DbType, ErrorKind, OracleNumber, OracleTimestamp, ToDbValue};
use serde_json::Value;
use url::Url;

use super::{Job, Outcome};
use crate::{
    config::AccessMode,
    error::{AppError, Effect, ErrorCode, Result},
    model::{self, BoundValue, Column, OutputBudget, StatementResult},
    policy::StatementKind,
};

pub async fn run(job: Job) -> Result<Outcome> {
    // Permits move into the blocking closure and remain held even if its caller times out.
    tokio::task::spawn_blocking(move || run_blocking(job))
        .await
        .map_err(|_| {
            AppError::new(
                ErrorCode::WriteOutcomeUnknown,
                "A tarefa Oracle encerrou sem resultado confirmado.",
            )
            .with_effect(Effect::Unknown)
        })?
}

fn run_blocking(job: Job) -> Result<Outcome> {
    let Job {
        config,
        statements,
        batch,
        progress,
        permits,
    } = job;
    let _permits = permits;
    // This driver flag prints packets, including SQL and authentication data, to stdout.
    if std::env::var_os("RSO_DEBUG_PACKETS").is_some() {
        return Err(AppError::new(
            ErrorCode::ConfigInvalid,
            "Remova RSO_DEBUG_PACKETS do ambiente para usar Oracle pelo MCP.",
        ));
    }
    let mut bindings = Vec::with_capacity(statements.len());
    for (index, statement) in statements.iter().enumerate() {
        let parameters = statement
            .parameters
            .iter()
            .map(bind)
            .collect::<Result<Vec<_>>>()
            .map_err(|mut error| {
                error.failed_statement_index = Some(index);
                error
            })?;
        bindings.push(parameters);
    }
    let deadline = Instant::now() + Duration::from_secs(config.limits.query_timeout_seconds);
    let url =
        Url::parse(config.url.expose()).map_err(|_| AppError::invalid("URL Oracle inválida."))?;
    let host = url
        .host_str()
        .ok_or_else(|| AppError::invalid("Host Oracle ausente."))?;
    let service = url.path().trim_start_matches('/');
    if !host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"._-:[]".contains(&b))
        || !service
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(AppError::invalid(
            "Host ou service name Oracle não suportado pela conexão simples.",
        ));
    }
    let timeout = config.limits.query_timeout_seconds;
    let descriptor = format!(
        "(DESCRIPTION=(CONNECT_TIMEOUT={timeout})(TRANSPORT_CONNECT_TIMEOUT={timeout})(RETRY_COUNT=0)(ADDRESS=(PROTOCOL=TCP)(HOST={host})(PORT={}))(CONNECT_DATA=(SERVICE_NAME={service})))",
        url.port().unwrap_or(1521)
    );
    let options = oracledb::Config::default()
        .set_credentials(
            config
                .username
                .as_ref()
                .map_or("", |secret| secret.expose()),
            config
                .password
                .as_ref()
                .map_or("", |secret| secret.expose()),
        )
        .set_connect_string(&descriptor)
        .map_err(|_| AppError::invalid("Endereço Oracle inválido."))?
        .set_transport_connect_timeout(Some(Duration::from_secs(timeout)))
        .set_stmtcachesize(0);
    let connection = oracledb::connect(options).map_err(|error| {
        let mut error = driver_error(error);
        error.code = ErrorCode::ConnectionFailed;
        error.message = "Não foi possível conectar ao Oracle.".into();
        error
    })?;
    connection
        .set_call_timeout(Some(Duration::from_secs(timeout)))
        .map_err(driver_error)?;
    connection
        .execute("ALTER SESSION SET NLS_NUMERIC_CHARACTERS='.,'", &[])
        .map_err(driver_error)?;
    connection
        .execute("ALTER SESSION SET TIME_ZONE='+00:00'", &[])
        .map_err(driver_error)?;
    if let Some(schema) = &config.default_schema {
        connection
            .execute(
                &format!(
                    "ALTER SESSION SET CURRENT_SCHEMA={}",
                    super::quote_identifier(schema)
                ),
                &[],
            )
            .map_err(driver_error)?;
    }
    if batch {
        preflight(&connection, &config, &statements, &progress, deadline)?;
    }
    let writes = statements.iter().any(|s| s.kind != StatementKind::Read);
    let ddl = statements.iter().any(|s| s.kind == StatementKind::Ddl);
    if !writes || config.access_mode == AccessMode::ReadOnly {
        connection
            .execute("SET TRANSACTION READ ONLY", &[])
            .map_err(driver_error)?;
    }
    let mut results = Vec::with_capacity(statements.len());
    let mut budget = OutputBudget::new(&config.limits);
    let mut routine_sent = false;
    for (index, (plan, parameters)) in statements.iter().zip(&bindings).enumerate() {
        let operation = (|| -> Result<StatementResult> {
            if progress.cancelled.load(Ordering::SeqCst) || Instant::now() >= deadline {
                return Err(AppError::new(
                    ErrorCode::ExecutionTimeout,
                    "Prazo Oracle excedido.",
                ));
            }
            connection
                .set_call_timeout(Some(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .max(Duration::from_millis(1)),
                ))
                .map_err(driver_error)?;
            let sql = plan.sql.trim_end().trim_end_matches(';');
            let references = parameters
                .iter()
                .map(|value| &**value as &dyn ToDbValue)
                .collect::<Vec<_>>();
            let mut statement = connection
                .statement(sql)
                .map_err(driver_error)?
                .fetch_array_size(1)
                .prefetch_rows(1)
                .fetch_lobs()
                .exclude_from_cache()
                .build()
                .map_err(driver_error)?;
            let mut result = StatementResult {
                statement_index: index,
                ..Default::default()
            };
            progress.mark_sent();
            routine_sent |= plan.kind == StatementKind::Routine;
            if statement.is_query() {
                let rows = statement.query(&references).map_err(driver_error)?;
                result.columns = rows
                    .columns()
                    .iter()
                    .map(|column| Column {
                        name: column.name().to_owned(),
                        native_type: column.data_type(),
                        encoding: oracle_encoding(column.db_type()).into(),
                    })
                    .collect();
                budget.columns(&result.columns)?;
                for row in rows {
                    if progress.cancelled.load(Ordering::SeqCst) || Instant::now() >= deadline {
                        return Err(AppError::new(
                            ErrorCode::ExecutionTimeout,
                            "Prazo Oracle excedido.",
                        ));
                    }
                    connection
                        .set_call_timeout(Some(
                            deadline
                                .saturating_duration_since(Instant::now())
                                .max(Duration::from_millis(1)),
                        ))
                        .map_err(driver_error)?;
                    let row = row.map_err(driver_error)?;
                    budget.push(&mut result, decode(&row)?)?;
                    if result.truncated {
                        break;
                    }
                }
            } else {
                let executed = statement.execute(&references).map_err(driver_error)?;
                result.affected_rows = Some(executed.rows_affected());
            }
            Ok(result)
        })();
        match operation {
            Ok(result) => results.push(result),
            Err(mut error) => {
                error.failed_statement_index = Some(index);
                let effect = rollback_effect(&connection, writes, ddl || routine_sent);
                return Err(error.with_effect(effect));
            }
        }
    }
    if progress.cancelled.load(Ordering::SeqCst) || Instant::now() >= deadline {
        let effect = rollback_effect(&connection, writes, ddl || routine_sent);
        return Err(AppError::new(
            ErrorCode::ExecutionTimeout,
            "Prazo Oracle excedido antes da confirmação.",
        )
        .with_effect(effect));
    }
    if writes {
        connection.commit().map_err(|_| {
            AppError::new(
                ErrorCode::WriteOutcomeUnknown,
                "Resultado do commit Oracle desconhecido; não repita automaticamente.",
            )
            .with_effect(Effect::Unknown)
        })?;
        progress.committed.store(true, Ordering::SeqCst);
    } else {
        connection.rollback().map_err(driver_error)?;
    }
    Ok(Outcome {
        results,
        effect: if writes {
            Effect::Committed
        } else {
            Effect::None
        },
    })
}

fn rollback_effect(connection: &Connection, writes: bool, uncertain: bool) -> Effect {
    let rolled_back = connection.rollback().is_ok();
    // A stored routine can commit or run an autonomous transaction internally.
    if uncertain || (writes && !rolled_back) {
        Effect::Unknown
    } else if writes {
        Effect::RolledBack
    } else {
        Effect::None
    }
}

fn preflight(
    connection: &Connection,
    config: &crate::config::DatabaseConfig,
    statements: &[crate::policy::PreparedStatement],
    progress: &super::Progress,
    deadline: Instant,
) -> Result<()> {
    if !statements.iter().any(|s| s.kind == StatementKind::Write) {
        return Ok(());
    }
    let owner: String = connection
        .query_row(
            "SELECT SYS_CONTEXT('USERENV','SESSION_USER') FROM dual",
            &[],
        )
        .map_err(driver_error)?
        .get(0)
        .map_err(driver_error)?;
    let schema = config.default_schema.as_deref().unwrap_or(&owner);
    for statement in statements.iter().filter(|s| s.kind == StatementKind::Write) {
        for relation in &statement.relations {
            if progress.cancelled.load(Ordering::SeqCst) || Instant::now() >= deadline {
                return Err(AppError::new(
                    ErrorCode::ExecutionTimeout,
                    "Prazo de validação do lote Oracle excedido.",
                ));
            }
            connection
                .set_call_timeout(Some(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .max(Duration::from_millis(1)),
                ))
                .map_err(driver_error)?;
            let parts = relation
                .split('.')
                .map(|part| {
                    if part.starts_with('"') {
                        part.trim_matches('"').replace("\"\"", "\"")
                    } else {
                        part.to_uppercase()
                    }
                })
                .collect::<Vec<_>>();
            let table = match parts.as_slice() {
                [table] if schema == owner => table,
                [specified_owner, table] if specified_owner == &owner => table,
                _ => {
                    return Err(AppError::new(
                        ErrorCode::BatchNotAtomic,
                        "Lotes Oracle exigem tabelas locais do próprio usuário.",
                    ));
                }
            };
            connection
                .execute(
                    &format!(
                        "LOCK TABLE {}.{} IN ROW SHARE MODE",
                        super::quote_identifier(&owner),
                        super::quote_identifier(table)
                    ),
                    &[],
                )
                .map_err(driver_error)?;
            let metadata = connection
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM user_tables WHERE table_name=:1),
                        (SELECT COUNT(*) FROM user_triggers WHERE table_name=:2),
                        (SELECT COUNT(*) FROM user_external_tables WHERE table_name=:3)
                 FROM dual",
                    &[table, table, table],
                )
                .map_err(driver_error)?;
            let tables: i64 = metadata.get(0).map_err(driver_error)?;
            let triggers: i64 = metadata.get(1).map_err(driver_error)?;
            let external: i64 = metadata.get(2).map_err(driver_error)?;
            if tables != 1 || triggers != 0 || external != 0 {
                return Err(AppError::new(
                    ErrorCode::BatchNotAtomic,
                    "Lotes Oracle exigem tabelas próprias, sem triggers ou armazenamento externo.",
                ));
            }
        }
    }
    Ok(())
}

fn driver_error(error: oracledb::Error) -> AppError {
    let mut result = AppError::new(
        ErrorCode::SqlError,
        "O Oracle rejeitou a operação; consulte database_code e o SQL enviado pelo cliente.",
    );
    match error.kind() {
        ErrorKind::DbError(database) => {
            if database.code() == 1031 {
                result.code = ErrorCode::DatabasePermissionDenied;
            }
            result.database_code = Some(format!("ORA-{:05}", database.code()));
        }
        ErrorKind::CallTimeoutExceeded => {
            result.code = ErrorCode::ExecutionTimeout;
            result.message = "Prazo de resposta Oracle excedido.".into();
        }
        ErrorKind::UnsupportedConversion(_, _)
        | ErrorKind::UnsupportedDbType(_)
        | ErrorKind::ValueWasNull
        | ErrorKind::OutOfRange(_) => {
            result.code = ErrorCode::SerializationError;
            result.message = "Valor Oracle não representável pelo adaptador.".into();
        }
        _ => {}
    }
    result
}

fn bind(value: &BoundValue) -> Result<Box<dyn ToDbValue>> {
    Ok(match value {
        BoundValue::Null => Box::new(Option::<String>::None),
        BoundValue::Boolean(value) => Box::new(i32::from(*value)),
        BoundValue::Integer(value) => Box::new(*value),
        BoundValue::Unsigned(value) => Box::new(*value),
        BoundValue::Decimal(value) => {
            let normalized = value.normalized();
            let (coefficient, scale) = normalized.as_bigint_and_exponent();
            let digits = coefficient.to_string().trim_start_matches('-').len();
            let exponent = digits as i64 - scale - 1;
            if digits > 38 || !(-130..126).contains(&exponent) {
                return Err(AppError::invalid(
                    "Decimal Oracle exige até 38 dígitos significativos e magnitude entre 1e-130 e 1e126.",
                ));
            }
            let number: OracleNumber = normalized
                .to_plain_string()
                .parse()
                .map_err(|_| AppError::invalid("Decimal incompatível com Oracle NUMBER."))?;
            Box::new(number)
        }
        BoundValue::Float(value) => Box::new(*value),
        BoundValue::String(value) => Box::new(value.clone()),
        BoundValue::Date(value) => Box::new(bind_timestamp(value.and_time(NaiveTime::MIN))?),
        BoundValue::Timestamp(value) => Box::new(bind_timestamp(*value)?),
        BoundValue::TimestampTz(value) => Box::new(bind_timestamp(value.naive_utc())?),
        BoundValue::Json(value) => Box::new(value.to_string()),
        BoundValue::Binary(value) => Box::new(value.clone()),
    })
}

fn bind_timestamp(value: NaiveDateTime) -> Result<OracleTimestamp> {
    if !(1..=9999).contains(&value.year()) || value.nanosecond() >= 1_000_000_000 {
        return Err(AppError::invalid(
            "Data Oracle exige ano de 1 a 9999 e não aceita leap second.",
        ));
    }
    Ok(OracleTimestamp::from(value))
}

fn oracle_encoding(native: &DbType) -> &'static str {
    if native == oracledb::DB_TYPE_NUMBER || native == oracledb::DB_TYPE_BINARY_INTEGER {
        "decimal_string"
    } else if native == oracledb::DB_TYPE_RAW {
        "base64"
    } else if native == oracledb::DB_TYPE_BINARY_DOUBLE || native == oracledb::DB_TYPE_BINARY_FLOAT
    {
        "number"
    } else if native == oracledb::DB_TYPE_BOOLEAN {
        "boolean"
    } else {
        "string"
    }
}

fn decode(row: &oracledb::Row) -> Result<Vec<Value>> {
    row.columns()
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let native = column.db_type();
            if native == oracledb::DB_TYPE_NUMBER
                || native == oracledb::DB_TYPE_BINARY_INTEGER
                || native == oracledb::DB_TYPE_VARCHAR
                || native == oracledb::DB_TYPE_NVARCHAR
                || native == oracledb::DB_TYPE_CHAR
                || native == oracledb::DB_TYPE_NCHAR
                || native == oracledb::DB_TYPE_ROWID
                || native == oracledb::DB_TYPE_UROWID
            {
                Ok(row
                    .get::<Option<String>>(index)
                    .map_err(driver_error)?
                    .map(Value::String)
                    .unwrap_or(Value::Null))
            } else if native == oracledb::DB_TYPE_RAW {
                Ok(row
                    .get::<Option<Vec<u8>>>(index)
                    .map_err(driver_error)?
                    .map(|bytes| model::binary(&bytes))
                    .unwrap_or(Value::Null))
            } else if native.is_date_type() {
                Ok(row
                    .get::<Option<OracleTimestamp>>(index)
                    .map_err(driver_error)?
                    .map(|timestamp| {
                        let text = timestamp.to_string();
                        Value::String(
                            if native == oracledb::DB_TYPE_TIMESTAMP_LTZ && !timestamp.is_utc() {
                                format!("{text}Z")
                            } else {
                                text
                            },
                        )
                    })
                    .unwrap_or(Value::Null))
            } else if native == oracledb::DB_TYPE_BOOLEAN {
                Ok(row
                    .get::<Option<bool>>(index)
                    .map_err(driver_error)?
                    .map(Value::Bool)
                    .unwrap_or(Value::Null))
            } else if native == oracledb::DB_TYPE_BINARY_FLOAT
                || native == oracledb::DB_TYPE_BINARY_DOUBLE
            {
                match row.get::<Option<f64>>(index).map_err(driver_error)? {
                    None => Ok(Value::Null),
                    Some(value) => serde_json::Number::from_f64(value)
                        .map(Value::Number)
                        .ok_or_else(|| {
                            AppError::new(ErrorCode::SerializationError, "Float Oracle não finito.")
                        }),
                }
            } else {
                Err(AppError::unsupported(
                    "LOB ou tipo Oracle não suportado na serialização limitada.",
                ))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Parameter;

    #[test]
    fn decimal_limits_reject_values_the_driver_cannot_encode_exactly() -> Result<()> {
        for text in [
            "1e126",
            "1e-131",
            "1e10000",
            "123456789012345678901234567890123456789",
        ] {
            let value = Parameter::Decimal(text.into()).resolve()?;
            assert!(matches!(bind(&value), Err(error) if error.code == ErrorCode::InvalidArgument));
        }
        Ok(())
    }

    #[test]
    fn decimal_boundaries_and_scientific_notation_are_accepted() -> Result<()> {
        for text in [
            "0",
            "1e-130",
            "9.9e125",
            "-1e-12",
            "12345678901234567890123456789012345678",
        ] {
            let value = Parameter::Decimal(text.into()).resolve()?;
            assert!(
                bind(&value).is_ok(),
                "Expected a representable Oracle NUMBER"
            );
        }
        Ok(())
    }

    #[test]
    fn timestamps_outside_the_driver_range_are_rejected() -> Result<()> {
        for text in [
            "0000-01-01T00:00:00",
            "+10000-01-01T00:00:00",
            "2026-12-31T23:59:60",
        ] {
            let value = Parameter::Timestamp(text.into()).resolve()?;
            assert!(matches!(bind(&value), Err(error) if error.code == ErrorCode::InvalidArgument));
        }
        Ok(())
    }
}
