use std::str::FromStr;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use bigdecimal::BigDecimal;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    config::ExecutionLimits,
    error::{AppError, ErrorCode, Result},
};

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Parameter {
    Null,
    Boolean(bool),
    Integer(String),
    Decimal(String),
    Float(f64),
    String(String),
    Date(String),
    Timestamp(String),
    Json(Value),
    Binary(String),
}

#[derive(Clone)]
pub enum BoundValue {
    Null,
    Boolean(bool),
    Integer(i64),
    Unsigned(u64),
    Decimal(BigDecimal),
    Float(f64),
    String(String),
    Date(NaiveDate),
    Timestamp(NaiveDateTime),
    TimestampTz(DateTime<Utc>),
    Json(Value),
    Binary(Vec<u8>),
}

impl Parameter {
    pub fn resolve(&self) -> Result<BoundValue> {
        let invalid = || AppError::invalid("Valor de parâmetro incompatível com o tipo declarado.");
        Ok(match self {
            Self::Null => BoundValue::Null,
            Self::Boolean(value) => BoundValue::Boolean(*value),
            Self::Integer(value) => {
                if let Ok(value) = value.parse::<i64>() {
                    BoundValue::Integer(value)
                } else {
                    BoundValue::Unsigned(value.parse().map_err(|_| invalid())?)
                }
            }
            Self::Decimal(value) => {
                let decimal = BigDecimal::from_str(value).map_err(|_| invalid())?;
                if decimal.as_bigint_and_exponent().1.unsigned_abs() > 10_000 {
                    return Err(invalid());
                }
                BoundValue::Decimal(decimal)
            }
            Self::Float(value) if value.is_finite() => BoundValue::Float(*value),
            Self::Float(_) => return Err(invalid()),
            Self::String(value) => BoundValue::String(value.clone()),
            Self::Date(value) => BoundValue::Date(
                NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| invalid())?,
            ),
            Self::Timestamp(value) => {
                if let Ok(timestamp) = DateTime::parse_from_rfc3339(value) {
                    BoundValue::TimestampTz(timestamp.with_timezone(&Utc))
                } else {
                    BoundValue::Timestamp(
                        NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                            .or_else(|_| {
                                NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
                            })
                            .map_err(|_| invalid())?,
                    )
                }
            }
            Self::Json(value) => BoundValue::Json(value.clone()),
            Self::Binary(value) => {
                BoundValue::Binary(STANDARD.decode(value).map_err(|_| invalid())?)
            }
        })
    }
}

#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SqlStatement {
    pub sql: String,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteRequest {
    pub database_id: String,
    pub sql: Option<String>,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    pub statements: Option<Vec<SqlStatement>>,
}

impl ExecuteRequest {
    pub fn into_statements(self, limits: &ExecutionLimits) -> Result<(Vec<SqlStatement>, bool)> {
        let (statements, batch) = match (self.sql, self.statements) {
            (Some(sql), None) => (
                vec![SqlStatement {
                    sql,
                    parameters: self.parameters,
                }],
                false,
            ),
            (None, Some(statements)) if self.parameters.is_empty() && !statements.is_empty() => {
                (statements, true)
            }
            _ => {
                return Err(AppError::invalid(
                    "Informe sql/parameters ou statements, exclusivamente.",
                ));
            }
        };
        if statements.len() > limits.max_batch_statements {
            return Err(AppError::new(
                ErrorCode::LimitExceeded,
                "Lote excede o limite de statements.",
            ));
        }
        let sql_bytes: usize = statements.iter().map(|s| s.sql.len()).sum();
        let parameters_bytes = statements
            .iter()
            .try_fold(0usize, |size, s| {
                serde_json::to_vec(&s.parameters).map(|v| size.saturating_add(v.len()))
            })
            .map_err(|_| AppError::invalid("Parâmetros inválidos."))?;
        if sql_bytes > limits.max_sql_bytes || parameters_bytes > limits.max_parameter_bytes {
            return Err(AppError::new(
                ErrorCode::LimitExceeded,
                "SQL ou parâmetros excedem o limite de entrada.",
            ));
        }
        Ok((statements, batch))
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListRequest {
    pub cursor: Option<String>,
}

#[derive(Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MetadataKind {
    #[default]
    Summary,
    Schemas,
    Tables,
    Columns,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeRequest {
    pub database_id: String,
    #[serde(default)]
    pub include: MetadataKind,
    pub schema: Option<String>,
    pub table: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Column {
    pub name: String,
    pub native_type: String,
    pub encoding: String,
}

#[derive(Debug, Default, Serialize)]
pub struct StatementResult {
    pub statement_index: usize,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    pub returned_rows: usize,
    pub affected_rows: Option<u64>,
    pub truncated: bool,
    pub limit_reason: Option<&'static str>,
}

/// One shared budget across every statement in a batch; leaves room for the envelope.
pub struct OutputBudget {
    remaining: usize,
    max_data_bytes: usize,
    max_rows: usize,
    used_rows: usize,
}

impl OutputBudget {
    pub fn new(limits: &ExecutionLimits) -> Self {
        Self {
            remaining: limits.max_result_bytes.saturating_sub(768),
            max_data_bytes: limits.max_result_bytes.saturating_sub(768),
            max_rows: limits.max_rows,
            used_rows: 0,
        }
    }

    pub fn columns(&mut self, columns: &[Column]) -> Result<()> {
        let bytes = serde_json::to_vec(columns)
            .map_err(|_| {
                AppError::new(
                    ErrorCode::SerializationError,
                    "Metadados não representáveis.",
                )
            })?
            .len();
        if bytes > self.remaining {
            return Err(AppError::new(
                ErrorCode::LimitExceeded,
                "Metadados excedem o orçamento de saída.",
            ));
        }
        self.remaining -= bytes;
        Ok(())
    }

    pub fn push(&mut self, result: &mut StatementResult, row: Vec<Value>) -> Result<()> {
        if result.truncated {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&row)
            .map_err(|_| AppError::new(ErrorCode::SerializationError, "Linha não representável."))?
            .len()
            + 1;
        if bytes > self.max_data_bytes {
            return Err(AppError::new(
                ErrorCode::LimitExceeded,
                "Uma linha excede o orçamento total de saída.",
            ));
        }
        let reason = if self.used_rows >= self.max_rows {
            Some("MAX_ROWS")
        } else if bytes > self.remaining {
            Some("MAX_RESULT_BYTES")
        } else {
            None
        };
        if let Some(reason) = reason {
            result.truncated = true;
            result.limit_reason = Some(reason);
            return Ok(());
        }
        self.remaining -= bytes;
        self.used_rows += 1;
        result.rows.push(row);
        result.returned_rows += 1;
        Ok(())
    }
}

pub fn integer(value: i64) -> Value {
    if (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&value) {
        Value::from(value)
    } else {
        Value::String(value.to_string())
    }
}

pub fn unsigned(value: u64) -> Value {
    if value <= 9_007_199_254_740_991 {
        Value::from(value)
    } else {
        Value::String(value.to_string())
    }
}

pub fn binary(value: &[u8]) -> Value {
    Value::String(STANDARD.encode(value))
}
