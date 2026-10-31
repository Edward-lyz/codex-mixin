use anyhow::{Context, Result, bail};
use codex_mixin::application::error::OperationError;
use codex_mixin::config::{
    ContextSettings, load_stored_config, mutate_stored_config, stored_config_path,
};
use codex_mixin::context::ContextEngine;
use codex_mixin::context::types::{SESSION_SEPARATOR, session_display_name};
use serde_json::{Value, json};

use super::args::{ContextCommand, ContextConfigCommand, ContextSessionsCommand};

struct ContextUpdate {
    compression_mode: Option<String>,
    enabled: Option<bool>,
    growth_threshold_tokens: Option<u64>,
    pressure_ratio: Option<f64>,
    urgent_ratio: Option<f64>,
    protected_messages: Option<usize>,
    protected_tokens: Option<u64>,
    max_internal_rounds: Option<usize>,
    max_tier: Option<u8>,
    guidance_only: Option<bool>,
    context_window_tokens: Option<String>,
}

pub(super) async fn run(command: ContextCommand) -> Result<()> {
    match command {
        ContextCommand::Config { command } => match command {
            ContextConfigCommand::Get { json: json_output } => config_get(json_output),
            ContextConfigCommand::Set {
                compression_mode,
                enabled,
                growth_threshold_tokens,
                pressure_ratio,
                urgent_ratio,
                protected_messages,
                protected_tokens,
                max_internal_rounds,
                max_tier,
                guidance_only,
                context_window_tokens,
                json: json_output,
            } => {
                config_set(
                    ContextUpdate {
                        compression_mode,
                        enabled,
                        growth_threshold_tokens,
                        pressure_ratio,
                        urgent_ratio,
                        protected_messages,
                        protected_tokens,
                        max_internal_rounds,
                        max_tier,
                        guidance_only,
                        context_window_tokens,
                    },
                    json_output,
                )
                .await
            }
        },
        ContextCommand::Status {
            session,
            json: json_output,
        } => status(session.as_deref(), json_output),
        ContextCommand::Stats { json: json_output } => stats(json_output),
        ContextCommand::Sessions { command } => match command {
            ContextSessionsCommand::List { json: json_output } => sessions_list(json_output),
            ContextSessionsCommand::Show {
                session,
                json: json_output,
            } => sessions_show(&session, json_output),
        },
        ContextCommand::Export { session, out } => export(session.as_deref(), out.as_deref()),
    }
}

fn config_get(json_output: bool) -> Result<()> {
    let settings = stored_settings()?;
    let defaults = ContextSettings::default();
    let compression_mode = if settings.growth_threshold_tokens < defaults.growth_threshold_tokens
        || settings.pressure_ratio < defaults.pressure_ratio
        || settings.urgent_ratio < defaults.urgent_ratio
    {
        "aggressive"
    } else {
        "gentle"
    };
    let value = json!({
        "context": settings,
        "compression_mode": compression_mode,
        "config_path": stored_config_path(),
        "database_path": codex_mixin::context::database_path()?,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        let settings = &value["context"];
        println!("context enabled: {}", settings["enabled"]);
        println!("compression mode: {compression_mode}");
        println!(
            "growth threshold tokens: {}",
            settings["growth_threshold_tokens"]
        );
        println!("pressure ratio: {}", settings["pressure_ratio"]);
        println!("urgent ratio: {}", settings["urgent_ratio"]);
        println!("protected messages: {}", settings["protected_messages"]);
        println!("protected tokens: {}", settings["protected_tokens"]);
        println!("max internal rounds: {}", settings["max_internal_rounds"]);
        println!("max tier: {}", settings["max_tier"]);
        println!("guidance only: {}", settings["guidance_only"]);
        println!(
            "context window tokens: {}",
            settings["context_window_tokens"]
        );
        println!("config path: {}", value["config_path"]);
        println!("database path: {}", value["database_path"]);
    }
    Ok(())
}

async fn config_set(update: ContextUpdate, json_output: bool) -> Result<()> {
    let (changed, settings) = mutate_stored_config(|stored| {
        let previous = stored.context.clone();
        apply_update(&mut stored.context, update)?;
        Ok((previous != stored.context, stored.context.clone()))
    })
    .context("update context settings")?;
    if changed {
        // Shells submit one save. Service selection and restart stay in the CLI,
        // and failures after the persisted write must never imply a rollback.
        let restarted = async {
            let runtime = super::runtime::load_runtime_metadata()?;
            let running = match runtime {
                Some(runtime) => super::runtime::pid_is_running(runtime.pid)?,
                None => false,
            };
            // Offline configuration remains offline; a live gateway must reload
            // the persisted engine settings exactly once before success.
            if running {
                super::progress_step("Restarting gateway to apply context settings");
                super::service::restart_managed().await?;
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        restarted.map_err(|source| OperationError::AfterCommit {
            stage: "context gateway restart",
            source,
        })?;
    }
    if json_output {
        let defaults = ContextSettings::default();
        let compression_mode = if settings.growth_threshold_tokens
            < defaults.growth_threshold_tokens
            || settings.pressure_ratio < defaults.pressure_ratio
            || settings.urgent_ratio < defaults.urgent_ratio
        {
            "aggressive"
        } else {
            "gentle"
        };
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"context": settings, "compression_mode": compression_mode})
            )?
        );
    } else {
        println!("context settings updated");
        println!("enabled: {}", settings.enabled);
        println!("max internal rounds: {}", settings.max_internal_rounds);
    }
    Ok(())
}

fn apply_update(settings: &mut ContextSettings, update: ContextUpdate) -> Result<()> {
    if let Some(mode) = update.compression_mode {
        let defaults = ContextSettings::default();
        (
            settings.growth_threshold_tokens,
            settings.pressure_ratio,
            settings.urgent_ratio,
        ) = match mode.as_str() {
            "aggressive" => (25_000, 0.6, 0.85),
            "gentle" => (
                defaults.growth_threshold_tokens,
                defaults.pressure_ratio,
                defaults.urgent_ratio,
            ),
            _ => bail!("context compression mode must be aggressive or gentle"),
        };
    }
    if let Some(enabled) = update.enabled {
        settings.enabled = enabled;
    }
    if let Some(value) = update.growth_threshold_tokens {
        settings.growth_threshold_tokens = value;
    }
    if let Some(value) = update.pressure_ratio {
        settings.pressure_ratio = value;
    }
    if let Some(value) = update.urgent_ratio {
        settings.urgent_ratio = value;
    }
    if let Some(value) = update.protected_messages {
        settings.protected_messages = value;
    }
    if let Some(value) = update.protected_tokens {
        settings.protected_tokens = value;
    }
    if let Some(value) = update.max_internal_rounds {
        settings.max_internal_rounds = value;
    }
    if let Some(value) = update.max_tier {
        settings.max_tier = value;
    }
    if let Some(value) = update.guidance_only {
        settings.guidance_only = value;
    }
    if let Some(value) = update.context_window_tokens {
        settings.context_window_tokens = parse_window(&value)?;
    }
    settings.validate()?;
    Ok(())
}

fn parse_window(value: &str) -> Result<Option<u64>> {
    let normalised = value.trim().to_ascii_lowercase();
    if matches!(normalised.as_str(), "none" | "off" | "default") {
        return Ok(None);
    }
    let tokens = normalised
        .parse::<u64>()
        .with_context(|| format!("invalid context window tokens: {value}"))?;
    if tokens == 0 {
        bail!("context window tokens must be positive");
    }
    Ok(Some(tokens))
}

fn status(session: Option<&str>, json_output: bool) -> Result<()> {
    let settings = stored_settings()?;
    let path = codex_mixin::context::database_path()?;
    if !path.exists() {
        let value = json!({
            "enabled": settings.enabled,
            "database_path": path,
            "sessions": [],
        });
        print_status_value(value, json_output);
        return Ok(());
    }
    let engine = open_engine(&settings)?;
    let value = match session {
        Some(input) => {
            let key = resolve_session(&engine, input)?;
            let window_tokens = settings.context_window_tokens.unwrap_or(0);
            serde_json::to_value(engine.status(&key, window_tokens, settings.enabled)?)?
        }
        None => {
            let sessions = engine.sessions()?;
            json!({
                "enabled": settings.enabled,
                "database_path": path,
                "sessions": sessions,
            })
        }
    };
    print_status_value(value, json_output);
    Ok(())
}

fn print_status_value(value: Value, json_output: bool) {
    if json_output {
        match serde_json::to_string_pretty(&value) {
            Ok(text) => println!("{text}"),
            Err(error) => eprintln!("failed to encode context status: {error}"),
        }
        return;
    }
    let Some(sessions) = value.get("sessions").and_then(Value::as_array) else {
        println!("session: {}", value["session"]);
        println!("revision: {}", value["revision"]);
        println!("estimated view tokens: {}", value["estimated_view_tokens"]);
        println!("pressure: {}", value["pressure"]);
        println!("blocks: {}", value["tiers"]);
        return;
    };
    if sessions.is_empty() {
        println!("no context sessions");
        return;
    }
    for session in sessions {
        let key = session["session"].as_str().unwrap_or_default();
        println!(
            "{}  messages={} blocks={} revision={}",
            session_display_name(key),
            session["messages"],
            session["blocks"],
            session["revision"]
        );
    }
}

fn stats(json_output: bool) -> Result<()> {
    let settings = stored_settings()?;
    let path = codex_mixin::context::database_path()?;
    let window_tokens = settings.context_window_tokens.unwrap_or(0);
    let stats = if path.exists() {
        open_engine(&settings)?.stats(window_tokens, settings.enabled)?
    } else {
        codex_mixin::context::ContextStats {
            enabled: settings.enabled,
            sessions: Vec::new(),
            totals: codex_mixin::context::StatsTotals::default(),
        }
    };
    let value = json!({
        "enabled": stats.enabled,
        "database_path": path,
        "totals": stats.totals,
        "sessions": stats.sessions,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!("context enabled: {}", stats.enabled);
    let totals = [
        stats.totals.sessions,
        stats.totals.compressions,
        stats.totals.saved_tokens,
        stats.totals.view_tokens,
        stats.totals.cache.turns,
        stats.totals.cache.input_tokens,
        stats.totals.cache.cached_tokens,
        stats.totals.cache.output_tokens,
    ]
    .map(|count| {
        super::COUNT_UNITS
            .iter()
            .find(|(scale, _)| count >= *scale - *scale / 20_000)
            .map_or_else(
                || count.to_string(),
                |(scale, suffix)| format!("{:.1}{suffix}", count as f64 / *scale as f64),
            )
    });
    println!(
        "sessions: {}  compressions: {}  saved tokens: {}  view tokens: {}",
        totals[0], totals[1], totals[2], totals[3]
    );
    println!(
        "cache: turns {}  hit {}  input {}  cached {}  output {}",
        totals[4],
        stats
            .totals
            .cache
            .hit_percent
            .map_or_else(|| "-".to_owned(), |value| format!("{value:.1}%")),
        totals[5],
        totals[6],
        totals[7]
    );
    for session in stats.sessions {
        let counts = [
            session.messages,
            session.compressions,
            session.saved_tokens,
            session.view_tokens,
        ]
        .map(|count| {
            super::COUNT_UNITS
                .iter()
                .find(|(scale, _)| count >= *scale - *scale / 20_000)
                .map_or_else(
                    || count.to_string(),
                    |(scale, suffix)| format!("{:.1}{suffix}", count as f64 / *scale as f64),
                )
        });
        println!(
            "{}  messages={} compressions={} saved_tokens={} view_tokens={} cache_hit={}",
            session.display,
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            session
                .cache
                .hit_percent
                .map_or_else(|| "-".to_owned(), |value| format!("{value:.1}%"))
        );
    }
    Ok(())
}

fn sessions_list(json_output: bool) -> Result<()> {
    let settings = stored_settings()?;
    let path = codex_mixin::context::database_path()?;
    let sessions = if path.exists() {
        open_engine(&settings)?.sessions()?
    } else {
        Vec::new()
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "database_path": path,
                "sessions": sessions,
            }))?
        );
    } else if sessions.is_empty() {
        println!("no context sessions");
    } else {
        for session in sessions {
            println!(
                "{}  messages={} blocks={} revision={} updated_at={}",
                session_display_name(&session.session),
                session.messages,
                session.blocks,
                session.revision,
                session.updated_at
            );
        }
    }
    Ok(())
}

fn sessions_show(session: &str, json_output: bool) -> Result<()> {
    let settings = stored_settings()?;
    let engine = open_engine(&settings)?;
    let key = resolve_session(&engine, session)?;
    let window_tokens = settings.context_window_tokens.unwrap_or(0);
    let status = engine.status(&key, window_tokens, settings.enabled)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("session: {}", session_display_name(&key));
        println!("revision: {}", status.revision);
        println!("enabled: {}", status.enabled);
        println!("window tokens: {}", status.window_tokens);
        println!("estimated view tokens: {}", status.estimated_view_tokens);
        println!("pressure: {}", status.pressure);
        println!("archive messages: {}", status.archive_messages);
        println!("archive bytes: {}", status.archive_bytes);
        println!("protected: {:?}", status.protected);
        println!("compressible: {:?}", status.compressible);
    }
    Ok(())
}

fn export(session: Option<&str>, out: Option<&std::path::Path>) -> Result<()> {
    let settings = stored_settings()?;
    let path = codex_mixin::context::database_path()?;
    let value = if !path.exists() {
        json!({"database_path": path, "sessions": []})
    } else {
        let engine = open_engine(&settings)?;
        match session {
            Some(input) => {
                let key = resolve_session(&engine, input)?;
                engine.export(&key)?
            }
            None => {
                let mut sessions = Vec::new();
                for summary in engine.sessions()? {
                    sessions.push(engine.export(&summary.session)?);
                }
                json!({"database_path": path, "sessions": sessions})
            }
        }
    };
    let text = serde_json::to_string_pretty(&value)?;
    match out {
        Some(path) => {
            std::fs::write(path, text + "\n")
                .with_context(|| format!("write context export {}", path.display()))?;
            println!("exported context to {}", path.display());
        }
        None => println!("{text}"),
    }
    Ok(())
}

fn stored_settings() -> Result<ContextSettings> {
    Ok(load_stored_config()?
        .map(|config| config.context)
        .unwrap_or_default())
}

fn open_engine(settings: &ContextSettings) -> Result<ContextEngine> {
    let path = codex_mixin::context::database_path()?;
    ContextEngine::open(settings.clone(), &path)
}

/// Accept either the raw session key or the display form printed by the list
/// command. Sessions never merge, so a missing key is an explicit error.
fn resolve_session(engine: &ContextEngine, input: &str) -> Result<String> {
    let sessions = engine.sessions()?;
    if let Some(found) = sessions.iter().find(|summary| summary.session == input) {
        return Ok(found.session.clone());
    }
    let wanted = input.replace(" / ", &SESSION_SEPARATOR.to_string());
    if let Some(found) = sessions.iter().find(|summary| summary.session == wanted) {
        return Ok(found.session.clone());
    }
    if let Some(found) = sessions
        .iter()
        .find(|summary| session_display_name(&summary.session) == input)
    {
        return Ok(found.session.clone());
    }
    bail!("context session not found: {input}")
}
