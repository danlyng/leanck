//! Operation/parameter pairing has one authoritative conversion in both directions.
use super::*;
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Health,
    Status,
    Diagnostics,
    Goal,
    Sorry,
    List,
    References,
    RestartFile,
    Cancel,
    Shutdown,
}

impl Operation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Health => op::HEALTH,
            Self::Status => op::STATUS,
            Self::Diagnostics => op::DIAGNOSTICS,
            Self::Goal => op::GOAL,
            Self::Sorry => op::SORRY,
            Self::List => op::LIST,
            Self::References => op::REFERENCES,
            Self::RestartFile => op::RESTART_FILE,
            Self::Cancel => op::CANCEL,
            Self::Shutdown => op::SHUTDOWN,
        }
    }
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Operation {
    type Err = RequestError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            op::HEALTH => Self::Health,
            op::STATUS => Self::Status,
            op::DIAGNOSTICS => Self::Diagnostics,
            op::GOAL => Self::Goal,
            op::SORRY => Self::Sorry,
            op::LIST => Self::List,
            op::REFERENCES => Self::References,
            op::RESTART_FILE => Self::RestartFile,
            op::CANCEL => Self::Cancel,
            op::SHUTDOWN => Self::Shutdown,
            other => return Err(RequestError::UnknownOperation(other.into())),
        })
    }
}

#[derive(Debug, Error)]
pub enum RequestError {
    #[error("unknown op: {0}")]
    UnknownOperation(String),
    #[error("bad params: {0}")]
    InvalidParams(#[source] serde_json::Error),
}

#[derive(Debug, Clone)]
pub enum Command {
    Health,
    Status,
    Diagnostics(DiagnosticsParams),
    Goal(GoalParams),
    Sorry(SorryParams),
    List(ListParams),
    References(ReferencesParams),
    RestartFile(RestartFileParams),
    Cancel(CancelParams),
    Shutdown,
}

impl Command {
    pub const fn operation(&self) -> Operation {
        match self {
            Self::Health => Operation::Health,
            Self::Status => Operation::Status,
            Self::Diagnostics(_) => Operation::Diagnostics,
            Self::Goal(_) => Operation::Goal,
            Self::Sorry(_) => Operation::Sorry,
            Self::List(_) => Operation::List,
            Self::References(_) => Operation::References,
            Self::RestartFile(_) => Operation::RestartFile,
            Self::Cancel(_) => Operation::Cancel,
            Self::Shutdown => Operation::Shutdown,
        }
    }

    pub fn into_request(
        self,
        id: String,
        timeout_ms: Option<u64>,
    ) -> Result<Request, serde_json::Error> {
        let operation = self.operation();
        let params = match self {
            Self::Health | Self::Status | Self::Shutdown => json!({}),
            Self::Diagnostics(params) => serde_json::to_value(params)?,
            Self::Goal(params) => serde_json::to_value(params)?,
            Self::Sorry(params) => serde_json::to_value(params)?,
            Self::List(params) => serde_json::to_value(params)?,
            Self::References(params) => serde_json::to_value(params)?,
            Self::RestartFile(params) => serde_json::to_value(params)?,
            Self::Cancel(params) => serde_json::to_value(params)?,
        };
        Ok(Request {
            id,
            op: operation.as_str().into(),
            timeout_ms,
            params,
        })
    }
}

impl Request {
    pub fn command(&self) -> Result<Command, RequestError> {
        fn parse<T: serde::de::DeserializeOwned>(params: &Value) -> Result<T, RequestError> {
            serde_json::from_value(params.clone()).map_err(RequestError::InvalidParams)
        }
        self.op.parse::<Operation>().and_then(|op| match op {
            Operation::Health => Ok(Command::Health),
            Operation::Status => Ok(Command::Status),
            Operation::Shutdown => Ok(Command::Shutdown),
            Operation::Diagnostics => parse(&self.params).map(Command::Diagnostics),
            Operation::Goal => parse(&self.params).map(Command::Goal),
            Operation::Sorry => parse(&self.params).map(Command::Sorry),
            Operation::List => parse(&self.params).map(Command::List),
            Operation::References => parse(&self.params).map(Command::References),
            Operation::RestartFile => parse(&self.params).map(Command::RestartFile),
            Operation::Cancel => parse(&self.params).map(Command::Cancel),
        })
    }
}
