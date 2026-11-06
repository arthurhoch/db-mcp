use std::{
    borrow::Cow,
    fmt::{Display, Write},
};

use bigdecimal::BigDecimal;
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use futures_util::TryStreamExt;
use serde_json::Value;
use tiberius::{
    AuthMethod, Client, ColumnData, ColumnType, Config, EncryptionLevel, FromSql, Query, QueryItem,
    Row, numeric::Numeric,
};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};
use url::Url;

use super::{Job, Outcome};
use crate::{
    config::DatabaseConfig,
    error::{AppError, Effect, ErrorCode, Result},
    model::{self, BoundValue, Column, OutputBudget, StatementResult},
    policy::PreparedStatement,
};

pub async fn run(job: Job) -> Result<Outcome> {
    let Job {
        config,
        statements,
        batch,
        explain,
        progress,
        permits,
    } = job;
    let _permits = permits;
    let mut connection = connect(&config).await?;
    if explain {
        command(&mut connection, "SET SHOWPLAN_XML ON").await?;
    } else if batch {
        command(&mut connection, "BEGIN TRANSACTION").await?;
    }

    let mut budget = OutputBudget::new(&config.limits);
    let mut results = Vec::with_capacity(statements.len());
    for (index, statement) in statements.iter().enumerate() {
        let result = async {
            if explain {
                let sql = plan_batch(statement)?;
                progress.mark_sent();
                let rows = connection.simple_query(sql).await.map_err(driver_error)?;
                let plan = collect(rows, index, &mut budget).await?;
                if plan.rows.is_empty() && !plan.truncated {
                    return Err(AppError::new(
                        ErrorCode::SqlError,
                        "O SQL Server não retornou um plano estimado.",
                    ));
                }
                return Ok(plan);
            }
            let mut query = Query::new(statement.sql.as_str());
            for value in &statement.parameters {
                bind(&mut query, value)?;
            }
            progress.mark_sent();
            let rows = query.query(&mut connection).await.map_err(driver_error)?;
            collect(rows, index, &mut budget).await
        }
        .await
        .map_err(|mut error| {
            error.failed_statement_index = Some(index);
            if progress.sent.load(std::sync::atomic::Ordering::SeqCst) {
                error = error.with_effect(Effect::None);
            }
            error
        })?;
        results.push(result);
    }
    if batch {
        command(&mut connection, "ROLLBACK TRANSACTION").await?;
    }
    // Dropping this connection also discards SHOWPLAN and any unconsumed rows.
    Ok(Outcome {
        results,
        effect: Effect::None,
    })
}

fn plan_batch(statement: &PreparedStatement) -> Result<String> {
    // SHOWPLAN returns a plan for a SQL batch, not for Tiberius's sp_executesql RPC.
    // Declare parameter types so the compiler can resolve placeholders without substituting values.
    let mut sql = String::new();
    for (index, parameter) in statement.parameters.iter().enumerate() {
        let kind = parameter_type(parameter)?;
        writeln!(sql, "DECLARE @P{} {kind};", index + 1)
            .map_err(|_| AppError::invalid("Não foi possível preparar os parâmetros do plano."))?;
    }
    sql.push_str(&statement.sql);
    Ok(sql)
}

fn parameter_type(value: &BoundValue) -> Result<Cow<'static, str>> {
    Ok(match value {
        BoundValue::Null | BoundValue::String(_) | BoundValue::Json(_) => "nvarchar(max)".into(),
        BoundValue::Boolean(_) => "bit".into(),
        BoundValue::Integer(_) => "bigint".into(),
        BoundValue::Unsigned(_) => "numeric(20,0)".into(),
        BoundValue::Decimal(value) => {
            let number = decimal(value)?;
            format!("numeric({},{})", number.precision(), number.scale()).into()
        }
        BoundValue::Float(_) => "float(53)".into(),
        BoundValue::Date(_) => "date".into(),
        BoundValue::Timestamp(_) => "datetime2".into(),
        BoundValue::TimestampTz(_) => "datetimeoffset".into(),
        BoundValue::Binary(_) => "varbinary(max)".into(),
    })
}

async fn connect(config: &DatabaseConfig) -> Result<Client<Compat<TcpStream>>> {
    let connection_error = || {
        AppError::new(
            ErrorCode::ConnectionFailed,
            "Não foi possível conectar ao SQL Server.",
        )
    };
    let address = Url::parse(config.url.expose()).map_err(|_| connection_error())?;
    let host = address.host_str().ok_or_else(connection_error)?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let database = percent_encoding::percent_decode_str(address.path().trim_start_matches('/'))
        .decode_utf8()
        .map_err(|_| AppError::invalid("Database SQL Server não é UTF-8 válido."))?;
    let port = address.port().unwrap_or(1433);
    let mut options = Config::new();
    options.host(host);
    options.port(port);
    options.database(database);
    options.authentication(AuthMethod::sql_server(
        config
            .username
            .as_ref()
            .map_or("", |secret| secret.expose()),
        config
            .password
            .as_ref()
            .map_or("", |secret| secret.expose()),
    ));
    options.encryption(EncryptionLevel::Required);
    if let Some(path) = &config.tls_ca_file {
        let certificate = tokio::fs::read(path).await.map_err(|_| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                "Não foi possível ler a CA do SQL Server.",
            )
        })?;
        options.trust_cert_ca_bundle(certificate);
    }
    let stream = TcpStream::connect((host, port))
        .await
        .map_err(|_| connection_error())?;
    stream.set_nodelay(true).map_err(|_| connection_error())?;
    Client::connect(options, stream.compat_write())
        .await
        .map_err(|error| {
            let mut result = connection_error();
            result.database_code = error.code().map(|code| code.to_string());
            result
        })
}

async fn command(connection: &mut Client<Compat<TcpStream>>, sql: &'static str) -> Result<()> {
    connection
        .simple_query(sql)
        .await
        .map_err(driver_error)?
        .into_results()
        .await
        .map_err(driver_error)?;
    Ok(())
}

async fn collect(
    mut rows: tiberius::QueryStream<'_>,
    index: usize,
    budget: &mut OutputBudget,
) -> Result<StatementResult> {
    let mut result = StatementResult {
        statement_index: index,
        ..Default::default()
    };
    while let Some(item) = rows.try_next().await.map_err(driver_error)? {
        match item {
            QueryItem::Metadata(metadata) => {
                if metadata.result_index() != 0 {
                    return Err(AppError::unsupported(
                        "SQL Server retornou mais de um conjunto de resultados.",
                    ));
                }
                result.columns = metadata
                    .columns()
                    .iter()
                    .map(|column| Column {
                        name: column.name().into(),
                        native_type: format!("{:?}", column.column_type()),
                        encoding: encoding(column.column_type()).into(),
                    })
                    .collect();
                budget.columns(&result.columns)?;
            }
            QueryItem::Row(row) => {
                let values = row
                    .cells()
                    .enumerate()
                    .map(|(index, (_, value))| decode(&row, index, value))
                    .collect::<Result<Vec<_>>>()?;
                budget.push(&mut result, values)?;
                if result.truncated {
                    break;
                }
            }
        }
    }
    Ok(result)
}

fn encoding(column: ColumnType) -> &'static str {
    match column {
        ColumnType::Int1
        | ColumnType::Int2
        | ColumnType::Int4
        | ColumnType::Int8
        | ColumnType::Intn => "integer_or_string",
        ColumnType::Decimaln | ColumnType::Numericn => "decimal_string",
        ColumnType::BigVarBin | ColumnType::BigBinary | ColumnType::Image => "base64",
        ColumnType::Bit | ColumnType::Bitn => "boolean",
        ColumnType::Float4
        | ColumnType::Float8
        | ColumnType::Floatn
        | ColumnType::Money
        | ColumnType::Money4 => "number",
        _ => "string",
    }
}

fn bind<'a>(query: &mut Query<'a>, value: &'a BoundValue) -> Result<()> {
    match value {
        BoundValue::Null => query.bind(Option::<&str>::None),
        BoundValue::Boolean(value) => query.bind(*value),
        BoundValue::Integer(value) => query.bind(*value),
        BoundValue::Unsigned(value) => query.bind(Numeric::new_with_scale(i128::from(*value), 0)),
        BoundValue::Decimal(value) => query.bind(decimal(value)?),
        BoundValue::Float(value) => query.bind(*value),
        BoundValue::String(value) => query.bind(value.as_str()),
        BoundValue::Date(value) => {
            validate_year(value.year())?;
            query.bind(*value);
        }
        BoundValue::Timestamp(value) => {
            validate_year(value.year())?;
            query.bind(*value);
        }
        BoundValue::TimestampTz(value) => {
            validate_year(value.year())?;
            query.bind(*value);
        }
        BoundValue::Json(value) => query.bind(value.to_string()),
        BoundValue::Binary(value) => query.bind(value.as_slice()),
    }
    Ok(())
}

fn decimal(value: &BigDecimal) -> Result<Numeric> {
    let value = value.normalized();
    let invalid =
        || AppError::invalid("Decimal SQL Server exige precisão e escala de até 38 dígitos.");
    let (coefficient, scale) = value.as_bigint_and_exponent();
    let digits = coefficient.to_string().trim_start_matches('-').len() as i64;
    if digits + scale.saturating_neg().max(0) > 38 || scale > 38 {
        return Err(invalid());
    }
    let (coefficient, scale) = if scale < 0 {
        value.with_scale(0).into_bigint_and_exponent()
    } else {
        (coefficient, scale)
    };
    let coefficient = coefficient
        .to_string()
        .parse::<i128>()
        .map_err(|_| invalid())?;
    let numeric = Numeric::new_with_scale(coefficient, scale as u8);
    // The driver counts a zero integer part as one digit when encoding precision.
    if numeric.precision() > 38 {
        return Err(AppError::invalid(
            "O driver SQL Server aceita até 37 casas decimais no parâmetro.",
        ));
    }
    Ok(numeric)
}

fn validate_year(year: i32) -> Result<()> {
    if !(1..=9999).contains(&year) {
        return Err(AppError::invalid(
            "Data SQL Server exige ano entre 1 e 9999.",
        ));
    }
    Ok(())
}

fn decode(row: &Row, index: usize, value: &ColumnData<'_>) -> Result<Value> {
    Ok(match value {
        ColumnData::U8(value) => value.map_or(Value::Null, Value::from),
        ColumnData::I16(value) => value.map_or(Value::Null, Value::from),
        ColumnData::I32(value) => value.map_or(Value::Null, Value::from),
        ColumnData::I64(value) => value.map_or(Value::Null, model::integer),
        ColumnData::F32(value) => value.map_or(Value::Null, Value::from),
        ColumnData::F64(value) => value.map_or(Value::Null, Value::from),
        ColumnData::Bit(value) => value.map_or(Value::Null, Value::from),
        ColumnData::String(value) => value.as_deref().map_or(Value::Null, Value::from),
        ColumnData::Guid(value) => {
            value.map_or(Value::Null, |value| Value::String(value.to_string()))
        }
        ColumnData::Binary(value) => value.as_deref().map_or(Value::Null, model::binary),
        ColumnData::Numeric(value) => value.map_or(Value::Null, |value| {
            Value::String(
                BigDecimal::new(value.value().into(), i64::from(value.scale())).to_plain_string(),
            )
        }),
        ColumnData::Xml(value) => value
            .as_deref()
            .map_or(Value::Null, |value| Value::String(value.to_string())),
        ColumnData::Date(_) => temporal::<NaiveDate>(row, index)?,
        ColumnData::Time(_) => temporal::<NaiveTime>(row, index)?,
        ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => {
            temporal::<NaiveDateTime>(row, index)?
        }
        ColumnData::DateTimeOffset(_) => row
            .try_get::<DateTime<Utc>, _>(index)
            .map_err(driver_error)?
            .map_or(Value::Null, |value| Value::String(value.to_rfc3339())),
    })
}

fn temporal<'a, T: FromSql<'a> + Display>(row: &'a Row, index: usize) -> Result<Value> {
    row.try_get::<T, _>(index)
        .map(|value| value.map_or(Value::Null, |value| Value::String(value.to_string())))
        .map_err(driver_error)
}

fn driver_error(error: tiberius::error::Error) -> AppError {
    let code = error.code();
    let mut result = if matches!(code, Some(229 | 262 | 297 | 916)) {
        AppError::new(
            ErrorCode::DatabasePermissionDenied,
            "O SQL Server recusou a permissão.",
        )
    } else if matches!(error, tiberius::error::Error::Conversion(_)) {
        AppError::new(
            ErrorCode::SerializationError,
            "Valor SQL Server não representável.",
        )
    } else {
        AppError::new(ErrorCode::SqlError, "O SQL Server rejeitou a operação.")
    };
    result.database_code = code.map(|code| code.to_string());
    result
}
