use crate::config::{StoredGatewayConfig, mutate_stored_config};

use super::error::OperationError;

pub fn commit_provider_change<T>(
    mutation: impl FnOnce(&mut StoredGatewayConfig) -> anyhow::Result<T>,
) -> Result<T, OperationError> {
    mutate_stored_config(mutation).map_err(|source| OperationError::BeforeCommit { source })
}

pub fn after_provider_commit<T>(
    stage: &'static str,
    action: impl FnOnce() -> anyhow::Result<T>,
) -> Result<T, OperationError> {
    action().map_err(|source| OperationError::AfterCommit { stage, source })
}

pub async fn after_provider_commit_async<T>(
    stage: &'static str,
    action: impl std::future::Future<Output = anyhow::Result<T>>,
) -> Result<T, OperationError> {
    action
        .await
        .map_err(|source| OperationError::AfterCommit { stage, source })
}
