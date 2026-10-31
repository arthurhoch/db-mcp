use std::{str::FromStr, sync::atomic::Ordering, time::Duration};

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use futures_util::TryStreamExt;
use serde_json::Value;
use sqlx::{
    Column as _, ConnectOptions, Connection, Executor, Row, SqlSafeStr, Statement as _, TypeInfo,
    ValueRef,
};

use super::{Job, Outcome};
use crate::{
    config::{AccessMode, DatabaseConfig, Engine},
    error::{AppError, Effect, ErrorCode, Result},
    model::{self, BoundValue, Column, OutputBudget, StatementResult},
    policy::{PreparedStatement, StatementKind},
};

fn driver_error(error: sqlx::Error) -> AppError {
    let mut result = AppError::new(
        ErrorCode::SqlError,
        "O banco rejeitou a operação; consulte database_code e o SQL enviado pelo cliente.",
    );
    if let sqlx::Error::Database(database) = &error {
        let code = database.code().map(|code| code.into_owned());
        if code
            .as_deref()
            .is_some_and(|code| ["42501", "1044", "1045", "1142", "1227"].contains(&code))
        {
            result.code = ErrorCode::DatabasePermissionDenied;
        }
        result.database_code =
            code.filter(|code| code.len() <= 16 && code.bytes().all(|b| b.is_ascii_alphanumeric()));
    } else if matches!(
        error,
        sqlx::Error::ColumnDecode { .. }
            | sqlx::Error::Decode(_)
            | sqlx::Error::TypeNotFound { .. }
    ) {
        result = AppError::new(
            ErrorCode::SerializationError,
            "Tipo de resultado não suportado ou valor não representável.",
        );
    }
    result
}

async fn connect_postgres(config: &DatabaseConfig) -> Result<sqlx::PgConnection> {
    let options = sqlx::postgres::PgConnectOptions::from_str(config.url.expose())
        .map_err(|_| AppError::invalid("URL PostgreSQL inválida."))?
        .username(config.username.as_ref().map_or("", |s| s.expose()))
        .password(config.password.as_ref().map_or("", |s| s.expose()))
        .disable_statement_logging();
    let mut connection = sqlx::PgConnection::connect_with(&options)
        .await
        .map_err(|_| {
            AppError::new(
                ErrorCode::ConnectionFailed,
                "Não foi possível conectar ao PostgreSQL.",
            )
        })?;
    if let Some(schema) = &config.default_schema {
        sqlx::query("SELECT set_config('search_path', $1, false)")
            .bind(super::quote_identifier(schema))
            .execute(&mut connection)
            .await
            .map_err(driver_error)?;
    }
    sqlx::query("SELECT set_config('statement_timeout', $1, false)")
        .bind(format!("{}s", config.limits.query_timeout_seconds))
        .execute(&mut connection)
        .await
        .map_err(driver_error)?;
    Ok(connection)
}

async fn connect_mysql(config: &DatabaseConfig) -> Result<sqlx::MySqlConnection> {
    let options = sqlx::mysql::MySqlConnectOptions::from_str(config.url.expose())
        .map_err(|_| AppError::invalid("URL MySQL inválida."))?
        .username(config.username.as_ref().map_or("", |s| s.expose()))
        .password(config.password.as_ref().map_or("", |s| s.expose()))
        .disable_statement_logging();
    sqlx::MySqlConnection::connect_with(&options)
        .await
        .map_err(|_| {
            AppError::new(
                ErrorCode::ConnectionFailed,
                "Não foi possível conectar ao MySQL.",
            )
        })
}

async fn connect_sqlite(config: &DatabaseConfig) -> Result<sqlx::SqliteConnection> {
    let path = config
        .sqlite_path
        .as_ref()
        .ok_or_else(|| AppError::invalid("Arquivo SQLite não configurado."))?;
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(config.access_mode == AccessMode::ReadOnly)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(config.limits.query_timeout_seconds))
        .pragma("trusted_schema", "OFF")
        .disable_statement_logging();
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .map_err(|_| {
            AppError::new(
                ErrorCode::ConnectionFailed,
                "Não foi possível abrir o arquivo SQLite existente.",
            )
        })?;
    if config.access_mode == AccessMode::ReadOnly {
        connection
            .execute("PRAGMA query_only=ON")
            .await
            .map_err(driver_error)?;
    }
    Ok(connection)
}

fn encoding(native_type: &str) -> &'static str {
    let native = native_type.to_ascii_uppercase();
    if native.contains("INT") {
        "integer_or_string"
    } else if native.contains("DECIMAL") || native.contains("NUMERIC") {
        "decimal_string"
    } else if native.contains("BLOB") || native.contains("BINARY") || native == "BYTEA" {
        "base64"
    } else if native.contains("JSON") {
        "json"
    } else if native.contains("BOOL") {
        "boolean"
    } else if native.contains("FLOAT") || native.contains("DOUBLE") || native.contains("REAL") {
        "number"
    } else {
        "string"
    }
}

macro_rules! runner {
    ($module:ident, $connection:ty, $connect:ident, $bind:ident, $decode:ident, $read_begin:expr, $write_begin:expr) => {
        pub mod $module {
            use super::*;

            pub async fn run(job: Job) -> Result<Outcome> {
                let Job {
                    config,
                    statements,
                    batch,
                    progress,
                    permits,
                } = job;
                let _permits = permits;
                let mut connection = $connect(&config).await?;
                let writes = statements.iter().any(|s| s.kind != StatementKind::Read);
                let implicit_ddl = config.engine == Engine::Mysql
                    && statements.iter().any(|s| s.kind == StatementKind::Ddl);
                if !implicit_ddl {
                    connection
                        .execute(if writes {
                            $write_begin
                        } else {
                            $read_begin
                        })
                        .await
                        .map_err(driver_error)?;
                }
                if batch {
                    connection.check_batch(&statements).await?;
                }
                let mut budget = OutputBudget::new(&config.limits);
                let mut results = Vec::with_capacity(statements.len());
                for (index, statement) in statements.iter().enumerate() {
                    let result = execute_one(
                        &mut connection,
                        statement,
                        index,
                        &progress,
                        &mut budget,
                        config.engine,
                    )
                    .await;
                    match result {
                        Ok(result) => results.push(result),
                        Err(mut error) => {
                            error.failed_statement_index = Some(index);
                            let effect = if implicit_ddl {
                                Effect::Unknown
                            } else if connection.execute("ROLLBACK").await.is_ok() {
                                // MySQL accepts ROLLBACK even when a single DML changed MyISAM.
                                // Only batch preflight establishes transactional storage there.
                                if writes && config.engine == Engine::Mysql && !batch {
                                    Effect::Unknown
                                } else if writes {
                                    Effect::RolledBack
                                } else {
                                    Effect::None
                                }
                            } else if writes {
                                Effect::Unknown
                            } else {
                                Effect::None
                            };
                            return Err(error.with_effect(effect));
                        }
                    }
                }
                if writes {
                    if !implicit_ddl {
                        connection.execute("COMMIT").await.map_err(|_| AppError::new(ErrorCode::WriteOutcomeUnknown, "Não foi possível confirmar o resultado do commit; não repita automaticamente.").with_effect(Effect::Unknown))?;
                    }
                    progress.committed.store(true, Ordering::SeqCst);
                } else {
                    connection.execute("ROLLBACK").await.map_err(driver_error)?;
                }
                // Every operation owns its connection. No state is returned to a pool.
                drop(connection);
                Ok(Outcome {
                    results,
                    effect: if writes {
                        Effect::Committed
                    } else {
                        Effect::None
                    },
                })
            }

            async fn execute_one(
                connection: &mut $connection,
                statement: &PreparedStatement,
                index: usize,
                progress: &super::super::Progress,
                budget: &mut OutputBudget,
                engine: Engine,
            ) -> Result<StatementResult> {
                let parameter_types =
                    <$connection as BatchPreflight>::parameter_types(&statement.parameters);
                let description = connection
                    .prepare_with(
                        sqlx::AssertSqlSafe(statement.sql.clone()).into_sql_str(),
                        &parameter_types,
                    )
                    .await
                    .map_err(driver_error)?;
                let columns = description
                    .columns()
                    .iter()
                    .map(|column| Column {
                        name: column.name().to_owned(),
                        native_type: column.type_info().name().to_owned(),
                        encoding: if engine == Engine::Sqlite {
                            "dynamic"
                        } else {
                            encoding(column.type_info().name())
                        }
                        .into(),
                    })
                    .collect::<Vec<_>>();
                budget.columns(&columns)?;
                let mut result = StatementResult {
                    statement_index: index,
                    columns,
                    ..Default::default()
                };
                let query = $bind(&statement.sql, &statement.parameters)?;
                progress.mark_sent();
                let mut rows = connection.fetch_many(query);
                while let Some(step) = rows.try_next().await.map_err(driver_error)? {
                    match step {
                        sqlx::Either::Left(done) => {
                            if statement.kind != StatementKind::Read {
                                result.affected_rows = Some(done.rows_affected());
                            }
                        }
                        sqlx::Either::Right(row) => {
                            if !result.truncated {
                                let decoded = $decode(&row)?;
                                budget.push(&mut result, decoded)?;
                            }
                            if result.truncated && statement.kind == StatementKind::Read {
                                break;
                            }
                        }
                    }
                }
                Ok(result)
            }
        }
    };
}

// Native preflights keep schema locks through the same transaction as the batch.
trait BatchPreflight: Connection {
    fn parameter_types(_: &[BoundValue]) -> Vec<<Self::Database as sqlx::Database>::TypeInfo> {
        Vec::new()
    }
    async fn check_batch(&mut self, statements: &[PreparedStatement]) -> Result<()>;
}

impl BatchPreflight for sqlx::PgConnection {
    fn parameter_types(values: &[BoundValue]) -> Vec<sqlx::postgres::PgTypeInfo> {
        values
            .iter()
            .map(|value| match value {
                BoundValue::Null => <PgNull as sqlx::Type<sqlx::Postgres>>::type_info(),
                BoundValue::Boolean(_) => <bool as sqlx::Type<sqlx::Postgres>>::type_info(),
                BoundValue::Integer(_) => <i64 as sqlx::Type<sqlx::Postgres>>::type_info(),
                BoundValue::Unsigned(_) | BoundValue::Decimal(_) => {
                    <bigdecimal::BigDecimal as sqlx::Type<sqlx::Postgres>>::type_info()
                }
                BoundValue::Float(_) => <f64 as sqlx::Type<sqlx::Postgres>>::type_info(),
                BoundValue::String(_) => <String as sqlx::Type<sqlx::Postgres>>::type_info(),
                BoundValue::Date(_) => <NaiveDate as sqlx::Type<sqlx::Postgres>>::type_info(),
                BoundValue::Timestamp(_) => {
                    <NaiveDateTime as sqlx::Type<sqlx::Postgres>>::type_info()
                }
                BoundValue::TimestampTz(_) => {
                    <DateTime<Utc> as sqlx::Type<sqlx::Postgres>>::type_info()
                }
                BoundValue::Json(_) => {
                    <sqlx::types::Json<Value> as sqlx::Type<sqlx::Postgres>>::type_info()
                }
                BoundValue::Binary(_) => <Vec<u8> as sqlx::Type<sqlx::Postgres>>::type_info(),
            })
            .collect()
    }
    async fn check_batch(&mut self, statements: &[PreparedStatement]) -> Result<()> {
        for statement in statements.iter().filter(|s| s.kind == StatementKind::Write) {
            for relation in &statement.relations {
                // relation is a parser-rendered ObjectName, never an arbitrary string input.
                self.execute(
                    sqlx::AssertSqlSafe(format!(
                        "LOCK TABLE {relation} IN SHARE ROW EXCLUSIVE MODE"
                    ))
                    .into_sql_str(),
                )
                .await
                .map_err(driver_error)?;
                let row = sqlx::query("SELECT c.relkind::text AS kind, EXISTS(SELECT 1 FROM pg_trigger t WHERE t.tgrelid=c.oid AND NOT t.tgisinternal) AS has_triggers FROM pg_class c WHERE c.oid=to_regclass($1)")
                    .bind(relation).fetch_optional(&mut *self).await.map_err(driver_error)?;
                let Some(row) = row else {
                    return Err(AppError::new(
                        ErrorCode::BatchNotAtomic,
                        "Tabela PostgreSQL do lote não verificável.",
                    ));
                };
                let kind: String = row.try_get("kind").map_err(driver_error)?;
                let triggers: bool = row.try_get("has_triggers").map_err(driver_error)?;
                if kind != "r" || triggers {
                    return Err(AppError::new(
                        ErrorCode::BatchNotAtomic,
                        "Lotes PostgreSQL exigem tabelas locais não particionadas e sem triggers de usuário.",
                    ));
                }
            }
        }
        Ok(())
    }
}
impl BatchPreflight for sqlx::SqliteConnection {
    async fn check_batch(&mut self, _: &[PreparedStatement]) -> Result<()> {
        Ok(())
    }
}
// Schema grants are patterns unless partial_revokes is enabled. Global grants
// alone cannot prove visibility when partial revokes may remove schema access.
const MYSQL_BATCH_TABLE_SQL: &str = r#"
SELECT ENGINE,
       (SELECT COUNT(*) FROM information_schema.TRIGGERS tr
        WHERE tr.EVENT_OBJECT_SCHEMA=t.TABLE_SCHEMA AND tr.EVENT_OBJECT_TABLE=t.TABLE_NAME) AS trigger_count,
       CAST((EXISTS(SELECT 1 FROM information_schema.USER_PRIVILEGES p
                    WHERE REPLACE(p.GRANTEE,CHAR(39),'')=CURRENT_USER()
                      AND NOT @@partial_revokes AND p.PRIVILEGE_TYPE='TRIGGER')
             OR EXISTS(SELECT 1 FROM information_schema.SCHEMA_PRIVILEGES p
                       WHERE REPLACE(p.GRANTEE,CHAR(39),'')=CURRENT_USER()
                         AND (CASE WHEN @@partial_revokes THEN p.TABLE_SCHEMA=t.TABLE_SCHEMA
                                   ELSE t.TABLE_SCHEMA LIKE p.TABLE_SCHEMA END)
                         AND p.PRIVILEGE_TYPE='TRIGGER')
             OR EXISTS(SELECT 1 FROM information_schema.TABLE_PRIVILEGES p
                       WHERE REPLACE(p.GRANTEE,CHAR(39),'')=CURRENT_USER()
                         AND p.TABLE_SCHEMA=t.TABLE_SCHEMA AND p.TABLE_NAME=t.TABLE_NAME
                         AND p.PRIVILEGE_TYPE='TRIGGER')) AS SIGNED) AS trigger_visible
FROM information_schema.TABLES t
WHERE TABLE_SCHEMA=COALESCE(?,DATABASE()) AND TABLE_NAME=?
"#;

impl BatchPreflight for sqlx::MySqlConnection {
    async fn check_batch(&mut self, statements: &[PreparedStatement]) -> Result<()> {
        for statement in statements.iter().filter(|s| s.kind == StatementKind::Write) {
            for relation in &statement.relations {
                let parts = relation
                    .split('.')
                    .map(|part| part.trim_matches('`').to_owned())
                    .collect::<Vec<_>>();
                let (schema, table) = match parts.as_slice() {
                    [table] => (None, table),
                    [schema, table] => (Some(schema.as_str()), table),
                    _ => {
                        return Err(AppError::new(
                            ErrorCode::BatchNotAtomic,
                            "Referência de tabela não verificável para lote MySQL.",
                        ));
                    }
                };
                let quoted = parts
                    .iter()
                    .map(|part| format!("`{}`", part.replace('`', "``")))
                    .collect::<Vec<_>>()
                    .join(".");
                self.execute(
                    sqlx::AssertSqlSafe(format!("SELECT 1 FROM {quoted} WHERE 1=0")).into_sql_str(),
                )
                .await
                .map_err(driver_error)?;
                let row = sqlx::query(MYSQL_BATCH_TABLE_SQL)
                    .bind(schema)
                    .bind(table)
                    .fetch_optional(&mut *self)
                    .await
                    .map_err(driver_error)?;
                let Some(row) = row else {
                    return Err(AppError::new(
                        ErrorCode::BatchNotAtomic,
                        "Não foi possível verificar o engine de uma tabela do lote.",
                    ));
                };
                let engine: Option<String> = row.try_get("ENGINE").map_err(driver_error)?;
                let triggers: i64 = row.try_get("trigger_count").map_err(driver_error)?;
                let visible: i64 = row.try_get("trigger_visible").map_err(driver_error)?;
                if !engine
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("InnoDB"))
                    || triggers != 0
                    || visible == 0
                {
                    return Err(AppError::new(
                        ErrorCode::BatchNotAtomic,
                        "Lotes MySQL exigem tabelas InnoDB sem triggers e privilégios que permitam verificar seus triggers.",
                    ));
                }
            }
        }
        Ok(())
    }
}

runner!(
    postgres,
    sqlx::PgConnection,
    connect_postgres,
    bind_postgres,
    decode_postgres,
    "BEGIN READ ONLY",
    "BEGIN"
);
runner!(
    mysql,
    sqlx::MySqlConnection,
    connect_mysql,
    bind_mysql,
    decode_mysql,
    "START TRANSACTION READ ONLY",
    "START TRANSACTION"
);
runner!(
    sqlite,
    sqlx::SqliteConnection,
    connect_sqlite,
    bind_sqlite,
    decode_sqlite,
    "BEGIN",
    "BEGIN"
);

fn bind_postgres<'q>(
    sql: &'q str,
    parameters: &[BoundValue],
) -> Result<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>> {
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()));
    for value in parameters {
        query = match value {
            BoundValue::Null => query.bind(PgNull),
            BoundValue::Boolean(v) => query.bind(*v),
            BoundValue::Integer(v) => query.bind(*v),
            BoundValue::Unsigned(v) => query.bind(bigdecimal::BigDecimal::from(*v)),
            BoundValue::Decimal(v) => query.bind(v.clone()),
            BoundValue::Float(v) => query.bind(*v),
            BoundValue::String(v) => query.bind(v.clone()),
            BoundValue::Date(v) => query.bind(*v),
            BoundValue::Timestamp(v) => query.bind(*v),
            BoundValue::TimestampTz(v) => query.bind(*v),
            BoundValue::Json(v) => query.bind(sqlx::types::Json(v.clone())),
            BoundValue::Binary(v) => query.bind(v.clone()),
        };
    }
    Ok(query)
}

struct PgNull;
impl sqlx::Type<sqlx::Postgres> for PgNull {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}
impl sqlx::Encode<'_, sqlx::Postgres> for PgNull {
    fn encode_by_ref(
        &self,
        _: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> std::result::Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        Ok(sqlx::encode::IsNull::Yes)
    }
}

fn bind_mysql<'q>(
    sql: &'q str,
    parameters: &[BoundValue],
) -> Result<sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>> {
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()));
    for value in parameters {
        query = match value {
            BoundValue::Null => query.bind(Option::<String>::None),
            BoundValue::Boolean(v) => query.bind(*v),
            BoundValue::Integer(v) => query.bind(*v),
            BoundValue::Unsigned(v) => query.bind(*v),
            BoundValue::Decimal(v) => query.bind(v.clone()),
            BoundValue::Float(v) => query.bind(*v),
            BoundValue::String(v) => query.bind(v.clone()),
            BoundValue::Date(v) => query.bind(*v),
            BoundValue::Timestamp(v) => query.bind(*v),
            BoundValue::TimestampTz(v) => query.bind(v.naive_utc()),
            BoundValue::Json(v) => query.bind(sqlx::types::Json(v.clone())),
            BoundValue::Binary(v) => query.bind(v.clone()),
        };
    }
    Ok(query)
}

fn bind_sqlite<'q>(
    sql: &'q str,
    parameters: &[BoundValue],
) -> Result<sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>> {
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()));
    for value in parameters {
        query = match value {
            BoundValue::Null => query.bind(Option::<String>::None),
            BoundValue::Boolean(v) => query.bind(*v),
            BoundValue::Integer(v) => query.bind(*v),
            BoundValue::Unsigned(v) => {
                return Err(AppError::unsupported(format!(
                    "SQLite não representa inteiro sem sinal acima de i64: {} dígitos.",
                    v.to_string().len()
                )));
            }
            BoundValue::Decimal(v) => query.bind(v.to_string()),
            BoundValue::Float(v) => query.bind(*v),
            BoundValue::String(v) => query.bind(v.clone()),
            BoundValue::Date(v) => query.bind(*v),
            BoundValue::Timestamp(v) => query.bind(*v),
            BoundValue::TimestampTz(v) => query.bind(v.to_rfc3339()),
            BoundValue::Json(v) => query.bind(v.to_string()),
            BoundValue::Binary(v) => query.bind(v.clone()),
        };
    }
    Ok(query)
}

fn decode_postgres(row: &sqlx::postgres::PgRow) -> Result<Vec<Value>> {
    row.columns()
        .iter()
        .enumerate()
        .map(|(index, column)| {
            if row.try_get_raw(index).map_err(driver_error)?.is_null() {
                return Ok(Value::Null);
            }
            let native = column.type_info().name();
            Ok(match native {
                "BOOL" => Value::from(row.try_get::<bool, _>(index).map_err(driver_error)?),
                "INT2" => model::integer(i64::from(
                    row.try_get::<i16, _>(index).map_err(driver_error)?,
                )),
                "INT4" => model::integer(i64::from(
                    row.try_get::<i32, _>(index).map_err(driver_error)?,
                )),
                "INT8" => model::integer(row.try_get(index).map_err(driver_error)?),
                "NUMERIC" => Value::String(
                    row.try_get::<bigdecimal::BigDecimal, _>(index)
                        .map_err(driver_error)?
                        .to_string(),
                ),
                "FLOAT4" => finite(f64::from(
                    row.try_get::<f32, _>(index).map_err(driver_error)?,
                ))?,
                "FLOAT8" => finite(row.try_get(index).map_err(driver_error)?)?,
                "BYTEA" => model::binary(&row.try_get::<Vec<u8>, _>(index).map_err(driver_error)?),
                "JSON" | "JSONB" => {
                    row.try_get::<sqlx::types::Json<Value>, _>(index)
                        .map_err(driver_error)?
                        .0
                }
                "DATE" => Value::String(
                    row.try_get::<NaiveDate, _>(index)
                        .map_err(driver_error)?
                        .to_string(),
                ),
                "TIME" => Value::String(
                    row.try_get::<NaiveTime, _>(index)
                        .map_err(driver_error)?
                        .to_string(),
                ),
                "TIMESTAMP" => Value::String(
                    row.try_get::<NaiveDateTime, _>(index)
                        .map_err(driver_error)?
                        .to_string(),
                ),
                "TIMESTAMPTZ" => Value::String(
                    row.try_get::<DateTime<Utc>, _>(index)
                        .map_err(driver_error)?
                        .to_rfc3339(),
                ),
                "UUID" => Value::String(
                    row.try_get::<uuid::Uuid, _>(index)
                        .map_err(driver_error)?
                        .to_string(),
                ),
                "TEXT" | "VARCHAR" | "CHAR" | "BPCHAR" | "NAME" | "UNKNOWN" => {
                    Value::String(row.try_get::<String, _>(index).map_err(driver_error)?)
                }
                _ => {
                    return Err(AppError::unsupported(
                        "Tipo PostgreSQL não suportado na serialização.",
                    ));
                }
            })
        })
        .collect()
}

fn decode_mysql(row: &sqlx::mysql::MySqlRow) -> Result<Vec<Value>> {
    row.columns()
        .iter()
        .enumerate()
        .map(|(index, column)| {
            if row.try_get_raw(index).map_err(driver_error)?.is_null() {
                return Ok(Value::Null);
            }
            let native = column.type_info().name();
            Ok(match native {
                n if n.contains("UNSIGNED") && n.contains("INT") => {
                    model::unsigned(row.try_get::<u64, _>(index).map_err(driver_error)?)
                }
                "TINYINT" => model::integer(i64::from(
                    row.try_get::<i8, _>(index).map_err(driver_error)?,
                )),
                "SMALLINT" => model::integer(i64::from(
                    row.try_get::<i16, _>(index).map_err(driver_error)?,
                )),
                "INT" | "MEDIUMINT" => model::integer(i64::from(
                    row.try_get::<i32, _>(index).map_err(driver_error)?,
                )),
                "BIGINT" => model::integer(row.try_get(index).map_err(driver_error)?),
                "DECIMAL" => Value::String(
                    row.try_get::<bigdecimal::BigDecimal, _>(index)
                        .map_err(driver_error)?
                        .to_string(),
                ),
                "FLOAT" => finite(f64::from(
                    row.try_get::<f32, _>(index).map_err(driver_error)?,
                ))?,
                "DOUBLE" => finite(row.try_get(index).map_err(driver_error)?)?,
                "DATE" => Value::String(
                    row.try_get::<NaiveDate, _>(index)
                        .map_err(driver_error)?
                        .to_string(),
                ),
                "TIME" => Value::String(
                    row.try_get::<NaiveTime, _>(index)
                        .map_err(driver_error)?
                        .to_string(),
                ),
                "DATETIME" | "TIMESTAMP" => Value::String(
                    row.try_get::<NaiveDateTime, _>(index)
                        .map_err(driver_error)?
                        .to_string(),
                ),
                "JSON" => {
                    row.try_get::<sqlx::types::Json<Value>, _>(index)
                        .map_err(driver_error)?
                        .0
                }
                n if n.contains("BLOB") || n.contains("BINARY") => {
                    model::binary(&row.try_get::<Vec<u8>, _>(index).map_err(driver_error)?)
                }
                "CHAR" | "VARCHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT" | "ENUM"
                | "SET" => Value::String(row.try_get::<String, _>(index).map_err(driver_error)?),
                _ => {
                    return Err(AppError::unsupported(
                        "Tipo MySQL não suportado na serialização.",
                    ));
                }
            })
        })
        .collect()
}

fn decode_sqlite(row: &sqlx::sqlite::SqliteRow) -> Result<Vec<Value>> {
    (0..row.len())
        .map(|index| {
            let raw = row.try_get_raw(index).map_err(driver_error)?;
            if raw.is_null() {
                return Ok(Value::Null);
            }
            Ok(match raw.type_info().name() {
                "INTEGER" | "INT" | "INT64" | "BOOLEAN" => model::integer(
                    row.try_get_unchecked::<i64, _>(index)
                        .map_err(driver_error)?,
                ),
                "REAL" | "FLOAT" => finite(
                    row.try_get_unchecked::<f64, _>(index)
                        .map_err(driver_error)?,
                )?,
                "BLOB" => model::binary(
                    &row.try_get_unchecked::<Vec<u8>, _>(index)
                        .map_err(driver_error)?,
                ),
                _ => Value::String(
                    row.try_get_unchecked::<String, _>(index)
                        .map_err(driver_error)?,
                ),
            })
        })
        .collect()
}

fn finite(value: f64) -> Result<Value> {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::SerializationError,
                "Float não finito não pode ser representado em JSON.",
            )
        })
}
