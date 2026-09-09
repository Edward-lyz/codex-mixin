use crate::config::{
    ensure_gateway_client_key, gateway_client_key_exists, revoke_gateway_client_key,
};
use crate::gateway_access::GatewayClient;

pub fn install_with_client_key<T>(
    client: GatewayClient,
    install: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let key_existed = gateway_client_key_exists(client)?;
    ensure_gateway_client_key(client)?;
    finish_client_install(client, key_existed, install())
}

pub async fn install_with_client_key_async<T>(
    client: GatewayClient,
    install: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let key_existed = gateway_client_key_exists(client)?;
    ensure_gateway_client_key(client)?;
    finish_client_install(client, key_existed, install.await)
}

fn finish_client_install<T>(
    client: GatewayClient,
    key_existed: bool,
    result: anyhow::Result<T>,
) -> anyhow::Result<T> {
    match result {
        Ok(change) => Ok(change),
        Err(error) if key_existed => Err(error),
        Err(error) => match revoke_gateway_client_key(client) {
            Ok(()) => Err(error),
            Err(revoke_error) => Err(anyhow::anyhow!(
                "{error:#}; gateway client key rollback also failed: {revoke_error:#}"
            )),
        },
    }
}
