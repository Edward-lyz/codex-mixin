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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn after_commit_reports_the_saved_config_and_failing_stage() {
        let error = OperationError::AfterCommit {
            stage: "model discovery",
            source: anyhow::anyhow!("upstream timed out"),
        };
        let rendered = error.to_string();
        assert!(
            rendered.contains("configuration was saved"),
            "after-commit errors must not imply nothing was saved: {rendered}"
        );
        assert!(
            rendered.contains("model discovery failed"),
            "after-commit errors must name the failing stage: {rendered}"
        );
        assert!(rendered.contains("upstream timed out"));
    }

    #[test]
    fn before_commit_preserves_the_original_chain() {
        let error = OperationError::BeforeCommit {
            source: anyhow::anyhow!("invalid key"),
        };
        assert_eq!(error.to_string(), "invalid key");
    }

    #[test]
    fn converts_into_anyhow_for_failure_exit() {
        let error: anyhow::Error = OperationError::AfterCommit {
            stage: "probe",
            source: anyhow::anyhow!("boom"),
        }
        .into();
        assert!(error.to_string().contains("configuration was saved"));
        let chain = error.chain().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(chain.last().map(String::as_str), Some("boom"));
    }
}
