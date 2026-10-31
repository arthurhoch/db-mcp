use serde::Serialize;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    ConfigInvalid,
    InvalidArgument,
    DatabaseNotAvailable,
    ReadOnlyViolation,
    OperationNotSupported,
    BatchNotAtomic,
    ConnectionFailed,
    DatabasePermissionDenied,
    SqlError,
    LimitExceeded,
    ExecutionTimeout,
    ExecutionCancelled,
    WriteOutcomeUnknown,
    SerializationError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    NotStarted,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    None,
    Committed,
    RolledBack,
    Unknown,
}

/// Public errors only contain messages created by this application, never raw driver errors.
#[derive(Clone, Debug, Error, Serialize)]
#[error("{message}")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip)]
    pub execution_state: ExecutionState,
    #[serde(skip)]
    pub effect: Effect,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_statement_index: Option<usize>,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            execution_state: ExecutionState::NotStarted,
            effect: Effect::None,
            retryable: false,
            database_code: None,
            failed_statement_index: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::OperationNotSupported, message)
    }

    pub fn with_effect(mut self, effect: Effect) -> Self {
        self.effect = effect;
        self.execution_state = if effect == Effect::Unknown {
            ExecutionState::Unknown
        } else {
            ExecutionState::Failed
        };
        self
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
