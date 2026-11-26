//! Operation-level errors for provider and client management use cases.

use std::fmt;

/// A provider-management operation failed either before the main config was
/// committed (`BeforeCommit`) or after it was already saved (`AfterCommit`,
/// carrying the failing stage). `AfterCommit` lets callers tell the user
/// "the configuration was saved, but this stage failed" instead of implying
/// that nothing happened.
#[derive(Debug)]
pub enum OperationError {
    BeforeCommit {
        source: anyhow::Error,
    },
    AfterCommit {
        stage: &'static str,
        source: anyhow::Error,
    },
}

impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OperationError::BeforeCommit { source } => write!(f, "{source:#}"),
            OperationError::AfterCommit { stage, source } => {
                write!(f, "configuration was saved, but {stage} failed: {source:#}")
            }
        }
    }
}

impl std::error::Error for OperationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BeforeCommit { source } | Self::AfterCommit { source, .. } => {
                Some(source.as_ref())
            }
        }
    }
}
