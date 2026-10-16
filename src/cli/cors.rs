use codex_mixin::config::{load_stored_config, mutate_stored_config};
use serde_json::json;

use super::args::CorsCommand;

pub(super) fn run(command: CorsCommand) -> anyhow::Result<()> {
    let (origins, changed, json_output) = match command {
        CorsCommand::Get { json } => (
            load_stored_config()?
                .unwrap_or_default()
                .cors_allowed_origins,
            false,
            json,
        ),
        CorsCommand::Set {
            origins,
            disable,
            json,
        } => {
            let origins = if disable { Vec::new() } else { origins };
            mutate_stored_config(|config| {
                config.cors_allowed_origins.clone_from(&origins);
                Ok(())
            })?;
            (origins, true, json)
        }
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "cors_allowed_origins": origins,
                "restart_required": changed,
            }))?
        );
    } else {
        println!(
            "CORS allowed origins: {}",
            if origins.is_empty() {
                "(disabled)".to_owned()
            } else {
                origins.join(", ")
            }
        );
        if changed {
            println!("Restart the running gateway to apply this configuration.");
        }
    }
    Ok(())
}
