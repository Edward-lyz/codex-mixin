//! Allowlisted panel actions and their CLI argument arrays.
//!
//! Every action maps to a fixed CLI command. User values are validated here
//! (the trust boundary), passed as `--flag=value` so they can never be read as
//! another flag, and secrets travel through the child environment only.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use super::cli::ChildSecret;

const MAX_VALUE_CHARS: usize = 16_384;
const MAX_LIST_ITEMS: usize = 512;
const MAX_BENCHMARK_TIMEOUT_SECONDS: u64 = 3_600;
const SECRET_ENV_PREFIX: &str = "CODEX_MIXIN_PANEL_SECRET_";
/// Literal values with this prefix would be expanded by the CLI from the
/// panel's own environment, so they are rejected.
const CLI_SECRET_REFERENCE: &str = "@env:";

#[derive(Debug, Deserialize)]
#[serde(tag = "action", content = "params", rename_all = "snake_case")]
pub(super) enum PanelAction {
    ServiceStart,
    ServiceStop,
    ServiceRestart,
    ServiceStopRestoreClients,
    ProviderEnable {
        id: String,
    },
    ProviderDisable {
        id: String,
    },
    ProviderRemove {
        id: String,
    },
    ProviderReorder {
        ids: Vec<String>,
    },
    ProviderTest {
        id: String,
    },
    ProviderDiscover {
        id: String,
    },
    ProviderProbe {
        id: String,
    },
    ProviderSelect {
        id: String,
        #[serde(default)]
        models: Vec<String>,
        #[serde(default)]
        model_contexts: BTreeMap<String, u64>,
        #[serde(default)]
        clear_model_contexts: Vec<String>,
    },
    ProviderAdd {
        preset: String,
        #[serde(default)]
        options: BTreeMap<String, OptionValue>,
    },
    ProviderUpdate {
        id: String,
        #[serde(default)]
        options: BTreeMap<String, OptionValue>,
    },
    BenchmarkStart {
        timeout_seconds: u64,
        #[serde(default)]
        target_output_tokens: Option<u64>,
        #[serde(default)]
        providers: Vec<String>,
        #[serde(default)]
        models: Vec<String>,
    },
    FusionSet {
        profile: Value,
        #[serde(default)]
        replace_id: Option<String>,
    },
    FusionDelete {
        #[serde(default)]
        id: Option<String>,
    },
    AgentConnect {
        id: String,
        #[serde(default)]
        mode: Option<AgentConnectMode>,
    },
    AgentDisconnect {
        id: String,
    },
    AgentUpgrade {
        id: String,
    },
    EchEnable,
    EchDisable,
    EchTest,
    /// Re-report every local session (DUCX repair path).
    ReportReplay,
    Update,
    DoctorFix,
    RefreshCodexCatalog,
}

/// Codex-only choice between keeping the official account and custom-only.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum AgentConnectMode {
    OfficialAccount,
    CustomOnly,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum OptionValue {
    Flag(bool),
    Text(String),
    List(Vec<String>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OptionKind {
    /// `--flag=value`.
    Text,
    /// `--flag=@env:NAME`; the value goes to the child environment.
    Secret,
    /// `--flag=true|false`.
    Bool,
    /// `--flag` when true, omitted when false.
    Switch,
    /// Repeated `--flag=value`.
    List,
}

/// Provider fields the web panel may set. Executable paths, header
/// environment mappings and gateway keys stay CLI-only on purpose.
const PROVIDER_ADD_OPTIONS: &[(&str, OptionKind)] = &[
    ("id", OptionKind::Text),
    ("oauth_timeout_seconds", OptionKind::Text),
    ("key", OptionKind::Secret),
    ("aws_access_key_id", OptionKind::Secret),
    ("aws_secret_access_key", OptionKind::Secret),
    ("aws_session_token", OptionKind::Secret),
    ("aws_region", OptionKind::Text),
    ("display_name", OptionKind::Text),
    ("base_url", OptionKind::Text),
    ("website_url", OptionKind::Text),
    ("image_generation_path", OptionKind::Text),
    ("quota_url", OptionKind::Text),
    ("quota_username", OptionKind::Text),
    ("quota_workspace_id", OptionKind::Text),
    ("quota_auth_cookie", OptionKind::Secret),
    ("quota_currency", OptionKind::Text),
    ("quota_parser", OptionKind::Text),
    ("model", OptionKind::List),
    ("auxiliary_model_upstream", OptionKind::Bool),
    ("baidu_auth_bridge", OptionKind::Text),
    ("baidu_code_report", OptionKind::Bool),
    ("protocol", OptionKind::Text),
    ("api_path", OptionKind::Text),
];

const PROVIDER_UPDATE_OPTIONS: &[(&str, OptionKind)] = &[
    ("key", OptionKind::Secret),
    ("clear_key", OptionKind::Switch),
    ("aws_access_key_id", OptionKind::Secret),
    ("aws_secret_access_key", OptionKind::Secret),
    ("aws_session_token", OptionKind::Secret),
    ("aws_region", OptionKind::Text),
    ("clear_aws_session_token", OptionKind::Switch),
    ("clear_aws_credentials", OptionKind::Switch),
    ("display_name", OptionKind::Text),
    ("base_url", OptionKind::Text),
    ("website_url", OptionKind::Text),
    ("image_generation_path", OptionKind::Text),
    ("clear_image_generation", OptionKind::Switch),
    ("quota_url", OptionKind::Text),
    ("clear_quota", OptionKind::Switch),
    ("quota_username", OptionKind::Text),
    ("quota_workspace_id", OptionKind::Text),
    ("clear_quota_workspace_id", OptionKind::Switch),
    ("quota_auth_cookie", OptionKind::Secret),
    ("clear_quota_auth_cookie", OptionKind::Switch),
    ("quota_currency", OptionKind::Text),
    ("quota_parser", OptionKind::Text),
    ("auxiliary_model_upstream", OptionKind::Bool),
    ("baidu_auth_bridge", OptionKind::Text),
    ("baidu_code_report", OptionKind::Bool),
    ("auto_review_model", OptionKind::Text),
    ("clear_auto_review_model", OptionKind::Switch),
    // Custom providers choose their wire protocol and matching API path.
    ("protocol", OptionKind::Text),
    ("api_path", OptionKind::Text),
];

/// A validated CLI invocation ready to run.
#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct CliInvocation {
    pub(super) args: Vec<String>,
    pub(super) secrets: Vec<ChildSecret>,
}

impl CliInvocation {
    fn fixed(args: &[&str]) -> Self {
        Self {
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            secrets: Vec::new(),
        }
    }

    fn push(&mut self, arg: impl Into<String>) {
        self.args.push(arg.into());
    }

    fn push_value(&mut self, flag: &str, value: &str) -> Result<(), String> {
        validate_value(flag, value)?;
        self.args.push(format!("--{flag}={value}"));
        Ok(())
    }

    fn push_secret(&mut self, flag: &str, value: &str) -> Result<(), String> {
        if value.chars().any(char::is_control) {
            return Err(format!("{flag} contains control characters"));
        }
        let env_name = format!("{SECRET_ENV_PREFIX}{}", self.secrets.len());
        self.args
            .push(format!("--{flag}={CLI_SECRET_REFERENCE}{env_name}"));
        self.secrets.push(ChildSecret {
            env_name,
            value: value.to_owned(),
        });
        Ok(())
    }

    fn push_id(&mut self, label: &str, id: &str) -> Result<(), String> {
        validate_positional(label, id)?;
        self.args.push(id.to_owned());
        Ok(())
    }

    fn push_options(
        &mut self,
        options: &BTreeMap<String, OptionValue>,
        allowed: &[(&str, OptionKind)],
    ) -> Result<(), String> {
        for (name, value) in options {
            let kind = allowed
                .iter()
                .find(|(allowed_name, _)| allowed_name == name)
                .map(|(_, kind)| *kind)
                .ok_or_else(|| format!("unsupported provider option: {name}"))?;
            let flag = name.replace('_', "-");
            match (kind, value) {
                (OptionKind::Text | OptionKind::Secret, OptionValue::Text(text))
                    if text.trim().is_empty() => {}
                (OptionKind::Text, OptionValue::Text(text)) => {
                    self.push_value(&flag, text.trim())?
                }
                (OptionKind::Secret, OptionValue::Text(text)) => {
                    self.push_secret(&flag, text.trim())?;
                }
                (OptionKind::Bool, OptionValue::Flag(enabled)) => {
                    self.push(format!("--{flag}={enabled}"));
                }
                (OptionKind::Switch, OptionValue::Flag(true)) => self.push(format!("--{flag}")),
                (OptionKind::Switch, OptionValue::Flag(false)) => {}
                (OptionKind::List, OptionValue::List(items)) => {
                    ensure_list_size(name, items)?;
                    for item in items {
                        self.push_value(&flag, item.trim())?;
                    }
                }
                _ => return Err(format!("provider option {name} has the wrong type")),
            }
        }
        Ok(())
    }
}

fn validate_value(label: &str, value: &str) -> Result<(), String> {
    if value.chars().count() > MAX_VALUE_CHARS {
        return Err(format!("{label} is too long"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{label} contains control characters"));
    }
    if value.starts_with(CLI_SECRET_REFERENCE) {
        return Err(format!("{label} cannot reference environment variables"));
    }
    Ok(())
}

/// Positional values (provider ids, model names) must not look like flags.
fn validate_positional(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{label} is empty"));
    }
    if value.starts_with('-') {
        return Err(format!("{label} cannot start with '-'"));
    }
    validate_value(label, value)
}

fn ensure_list_size(label: &str, items: &[String]) -> Result<(), String> {
    if items.len() > MAX_LIST_ITEMS {
        return Err(format!("{label} has too many entries"));
    }
    Ok(())
}

/// `agent <operation> <id>`; the CLI validates the id against its registry.
fn agent_command(operation: &str, id: &str) -> Result<CliInvocation, String> {
    let mut invocation = CliInvocation::fixed(&["agent", operation]);
    invocation.push_id("agent id", id)?;
    Ok(invocation)
}

fn provider_id_command(operation: &str, id: &str) -> Result<CliInvocation, String> {
    let mut invocation = CliInvocation::fixed(&["providers", operation]);
    invocation.push_id("provider id", id)?;
    Ok(invocation)
}

impl PanelAction {
    /// Whether stdout is a JSON document the panel should parse.
    pub(super) fn prints_json(&self) -> bool {
        matches!(
            self,
            Self::ProviderTest { .. } | Self::EchTest | Self::ReportReplay
        )
    }

    /// CLI calls in order; a failure stops the sequence. Only the manual
    /// report runs several steps, mirroring the macOS app: reset upload
    /// credentials, restart for a fresh DUCX warmup, then replay sessions.
    pub(super) fn invocations(&self) -> Result<Vec<CliInvocation>, String> {
        if matches!(self, Self::ReportReplay) {
            return Ok(vec![
                CliInvocation::fixed(&["report-replay", "--prepare-warmup"]),
                CliInvocation::fixed(&["service", "restart"]),
                CliInvocation::fixed(&["report-replay", "--all-sessions", "--json"]),
            ]);
        }
        Ok(vec![self.invocation()?])
    }

    pub(super) fn invocation(&self) -> Result<CliInvocation, String> {
        match self {
            Self::ServiceStart => Ok(CliInvocation::fixed(&["service", "start"])),
            Self::ServiceStop => Ok(CliInvocation::fixed(&["service", "stop"])),
            Self::ServiceRestart => Ok(CliInvocation::fixed(&["service", "restart"])),
            Self::ServiceStopRestoreClients => Ok(CliInvocation::fixed(&[
                "service",
                "stop",
                "--restore-clients",
            ])),
            Self::ProviderEnable { id } => provider_id_command("enable", id),
            Self::ProviderDisable { id } => provider_id_command("disable", id),
            Self::ProviderRemove { id } => provider_id_command("remove", id),
            Self::ProviderDiscover { id } => provider_id_command("discover", id),
            Self::ProviderProbe { id } => provider_id_command("probe", id),
            Self::ProviderTest { id } => {
                let mut invocation = provider_id_command("test", id)?;
                invocation.push("--json");
                Ok(invocation)
            }
            Self::ProviderReorder { ids } => {
                if ids.is_empty() {
                    return Err("provider order is empty".to_owned());
                }
                ensure_list_size("provider order", ids)?;
                let mut invocation = CliInvocation::fixed(&["providers", "reorder"]);
                for id in ids {
                    invocation.push_id("provider id", id)?;
                }
                Ok(invocation)
            }
            Self::ProviderSelect {
                id,
                models,
                model_contexts,
                clear_model_contexts,
            } => {
                let mut invocation = provider_id_command("select", id)?;
                ensure_list_size("models", models)?;
                ensure_list_size("clear model contexts", clear_model_contexts)?;
                for model in models {
                    validate_positional("model", model)?;
                    invocation.push_value("model", model)?;
                }
                for (model, tokens) in model_contexts {
                    validate_positional("model", model)?;
                    invocation.push_value("model-context", &format!("{model}={tokens}"))?;
                }
                for model in clear_model_contexts {
                    validate_positional("model", model)?;
                    invocation.push_value("clear-model-context", model)?;
                }
                Ok(invocation)
            }
            Self::ProviderAdd { preset, options } => {
                let mut invocation = CliInvocation::fixed(&["providers", "add"]);
                validate_positional("preset", preset)?;
                invocation.push_value("preset", preset)?;
                invocation.push_options(options, PROVIDER_ADD_OPTIONS)?;
                Ok(invocation)
            }
            Self::ProviderUpdate { id, options } => {
                let mut invocation = provider_id_command("update", id)?;
                invocation.push_options(options, PROVIDER_UPDATE_OPTIONS)?;
                Ok(invocation)
            }
            Self::BenchmarkStart {
                timeout_seconds,
                target_output_tokens,
                providers,
                models,
            } => {
                if !(1..=MAX_BENCHMARK_TIMEOUT_SECONDS).contains(timeout_seconds) {
                    return Err(format!(
                        "timeout must be between 1 and {MAX_BENCHMARK_TIMEOUT_SECONDS} seconds"
                    ));
                }
                let mut invocation = CliInvocation::fixed(&["benchmark", "start"]);
                invocation.push(format!("--timeout-seconds={timeout_seconds}"));
                if let Some(tokens) = target_output_tokens {
                    invocation.push(format!("--target-output-tokens={tokens}"));
                }
                ensure_list_size("providers", providers)?;
                ensure_list_size("models", models)?;
                for provider in providers {
                    validate_positional("provider id", provider)?;
                    invocation.push_value("provider", provider)?;
                }
                for model in models {
                    validate_positional("model", model)?;
                    invocation.push_value("model", model)?;
                }
                Ok(invocation)
            }
            Self::FusionSet {
                profile,
                replace_id,
            } => {
                if !profile.is_object() {
                    return Err("fusion profile must be a JSON object".to_owned());
                }
                let mut invocation = CliInvocation::fixed(&["fusion", "set"]);
                invocation.push_value("profile-json", &profile.to_string())?;
                if let Some(id) = replace_id {
                    validate_positional("fusion profile id", id)?;
                    invocation.push_value("replace-id", id)?;
                }
                Ok(invocation)
            }
            Self::FusionDelete { id } => {
                let mut invocation = CliInvocation::fixed(&["fusion", "delete"]);
                if let Some(id) = id {
                    validate_positional("fusion profile id", id)?;
                    invocation.push_value("id", id)?;
                }
                Ok(invocation)
            }
            Self::AgentConnect { id, mode } => {
                let mut invocation = agent_command("connect", id)?;
                match mode {
                    Some(AgentConnectMode::OfficialAccount) => {
                        invocation.push("--mode=official-account")
                    }
                    Some(AgentConnectMode::CustomOnly) => invocation.push("--mode=custom-only"),
                    None => {}
                }
                Ok(invocation)
            }
            Self::AgentDisconnect { id } => agent_command("disconnect", id),
            Self::AgentUpgrade { id } => agent_command("upgrade", id),
            Self::EchEnable => Ok(CliInvocation::fixed(&["ech", "enable"])),
            Self::EchDisable => Ok(CliInvocation::fixed(&["ech", "disable"])),
            Self::EchTest => Ok(CliInvocation::fixed(&["ech", "test", "--json"])),
            Self::ReportReplay => {
                Err("manual report runs several steps; use invocations()".to_owned())
            }
            Self::Update => Ok(CliInvocation::fixed(&["update"])),
            Self::DoctorFix => Ok(CliInvocation::fixed(&["doctor", "--fix", "--quick"])),
            Self::RefreshCodexCatalog => Ok(CliInvocation::fixed(&["refresh-codex-catalog"])),
        }
    }
}
