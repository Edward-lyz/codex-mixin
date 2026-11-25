//! Typed views of the CLI JSON contract. Fields the shell does not use are
//! ignored; required fields fail decoding so unknown states never render as
//! "off" or "zero".

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Deserialize)]
pub struct PluginStatus {
    pub node_available: bool,
    pub node_version: Option<String>,
    pub runtime_initialized: bool,
    pub pi_version: Option<String>,
    pub installed_packages: Vec<String>,
    pub discovered_providers: Vec<String>,
    #[serde(default)]
    pub provider_details: Vec<PluginProvider>,
    pub discovered_models: Vec<String>,
    #[serde(default)]
    pub commands: Vec<PluginCommand>,
    pub npm_registry: String,
    #[serde(default)]
    pub extension_errors: usize,
    #[serde(alias = "runtime_error")]
    pub bridge_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PluginProvider {
    pub id: String,
    #[serde(default)]
    pub mixin_provider_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub model_count: usize,
    #[serde(default)]
    pub auth_status: PluginAuthStatus,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginAuthStatus {
    Configured,
    NotConfigured,
    PluginManaged,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
pub struct PluginInfo {
    pub version: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub readme: Option<gpui_kit::SharedString>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PluginCommandArgument {
    pub value: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct MarketPackage {
    pub name: String,
    pub version: Option<String>,
    pub description: String,
    pub installed: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PluginCommand {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub arguments: Vec<PluginCommandArgument>,
}

pub struct PluginData {
    pub status: PluginStatus,
    pub commands: Vec<PluginCommand>,
    pub commands_error: Option<String>,
}

#[derive(Deserialize)]
pub struct PluginCommandNotification {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: gpui_kit::SharedString,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginEvent {
    AuthUrl {
        url: String,
        instructions: Option<String>,
    },
    DeviceCode {
        user_code: String,
        verification_uri: String,
    },
    #[serde(alias = "info")]
    Progress {
        message: String,
    },
    Notify {
        level: String,
        message: String,
    },
    Prompt {
        id: String,
        prompt: PluginPrompt,
    },
    PromptCancelled {
        id: String,
    },
    Result {
        #[serde(flatten)]
        output: PluginEventResult,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum PluginEventResult {
    Login {
        provider: String,
        synced_models: usize,
    },
    Command {
        name: String,
        notifications: Vec<PluginCommandNotification>,
    },
}

#[derive(Deserialize)]
pub struct PluginPrompt {
    #[serde(rename = "type")]
    pub kind: PluginPromptKind,
    pub message: String,
    pub placeholder: Option<String>,
    pub prefill: Option<String>,
    #[serde(default)]
    pub options: Vec<PluginPromptOption>,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginPromptKind {
    Text,
    Secret,
    Select,
    ManualCode,
    Confirm,
    Editor,
}

#[derive(Deserialize)]
pub struct PluginPromptOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Autostart {
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServiceStatus {
    pub configured: bool,
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub gateway_version: Option<String>,
    #[serde(default)]
    pub provider_readiness: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub official_ech: Option<EchStatus>,
    #[serde(default)]
    providers: ServiceProviders,
}

impl ServiceStatus {
    pub fn is_running(&self) -> bool {
        self.gateway.as_deref() == Some("running")
    }

    pub fn degraded_providers(&self) -> usize {
        match &self.providers {
            ServiceProviders::List(providers) => providers
                .iter()
                .filter(|provider| provider.enabled && provider.readiness.status == "degraded")
                .count(),
            ServiceProviders::Summary { degraded, .. } => *degraded,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum ServiceProviders {
    List(Vec<ServiceProvider>),
    Summary {
        #[serde(rename = "total")]
        _total: usize,
        #[serde(rename = "healthy")]
        _healthy: usize,
        degraded: usize,
        #[serde(rename = "disabled")]
        _disabled: usize,
    },
}

impl Default for ServiceProviders {
    fn default() -> Self {
        Self::List(Vec::new())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServiceProvider {
    pub enabled: bool,
    pub readiness: ServiceReadiness,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServiceReadiness {
    pub status: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EchStatus {
    pub enabled: bool,
    #[serde(default)]
    pub fallback_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ProviderList {
    pub providers: Vec<Provider>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Provider {
    pub id: String,
    pub display_name: String,
    pub kind: String,
    pub enabled: bool,
    pub readiness: String,
    pub icon: String,
    pub protocol: String,
    pub selected_models: Vec<String>,
    pub cached_models: Vec<ProviderModel>,
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub website_url: Option<String>,
    #[serde(default)]
    pub api_path: Option<String>,
    #[serde(default)]
    pub image_generation_path: Option<String>,
    #[serde(default)]
    pub api_key_configured: bool,
    #[serde(default)]
    pub model_order: Option<String>,
    #[serde(default)]
    pub model_context_overrides: BTreeMap<String, u64>,
    #[serde(default)]
    pub models_refreshed_at_ms: Option<i64>,
    #[serde(default)]
    pub last_model_refresh_error: Option<String>,
    #[serde(default)]
    pub unavailable_selected_models: Vec<String>,
    #[serde(default)]
    pub model_refresh: Option<ModelRefresh>,
    #[serde(default)]
    pub quota_username: Option<String>,
    #[serde(default)]
    pub quota_workspace_id: Option<String>,
    #[serde(default)]
    pub aws_region: Option<String>,
    #[serde(default)]
    pub baidu_auth_bridge: Option<String>,
    #[serde(default)]
    pub baidu_code_report: bool,
    #[serde(default)]
    pub auto_review_model: Option<String>,
    #[serde(default)]
    pub codex_thread_title_model: Option<String>,
    #[serde(default)]
    pub plugin_managed: bool,
    #[serde(default)]
    pub pi_plugin_package: Option<String>,
    #[serde(default)]
    pub pi_provider_id: Option<String>,
}

impl Provider {
    pub fn is_official(&self) -> bool {
        self.kind == "official"
    }

    pub fn is_healthy(&self) -> bool {
        self.enabled && self.readiness == "healthy"
    }
}

/// Background model-list refresh policy for one provider.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ModelRefresh {
    pub enabled: bool,
    pub interval_seconds: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ProviderModel {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub source_context_window: Option<u64>,
    #[serde(default)]
    pub supports_image: Option<bool>,
    #[serde(default)]
    pub supports_function_tools: Option<bool>,
    #[serde(default)]
    pub supports_thinking: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Interface {
    pub paths: InterfacePaths,
    pub provider_catalog: ProviderCatalog,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct InterfacePaths {
    pub state: String,
    pub gateway_log: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProviderCatalog {
    pub default_preset: String,
    pub default_aws_region: String,
    pub oauth_timeout_seconds: u64,
    pub presets: Vec<ProviderPreset>,
    #[serde(default)]
    pub protocols: Vec<ProtocolOption>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProviderPreset {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub icon: String,
    pub auth_mode: String,
    pub allow_multiple: bool,
    pub api_key_required: bool,
    pub custom_endpoint: bool,
    pub quota_credentials_required: bool,
    pub quota_username_required: bool,
    #[serde(default)]
    pub browser_oauth: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProtocolOption {
    pub id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct QuotaEntry {
    #[serde(default)]
    pub quota_id: Option<String>,
    pub label: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub limit: Option<f64>,
    #[serde(default)]
    pub used: Option<f64>,
    #[serde(default)]
    pub remaining: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub reset_at: Option<String>,
    #[serde(default)]
    pub window_seconds: Option<u64>,
}

impl QuotaEntry {
    /// Remaining fraction in `0..=1` when the quota has a known limit.
    pub fn remaining_fraction(&self) -> Option<f32> {
        if self.error.is_some() {
            return None;
        }
        let limit = self
            .limit
            .or_else(|| (self.unit.as_deref() == Some("percent")).then_some(100.0))
            .filter(|limit| *limit > 0.0)?;
        let remaining = self
            .remaining
            .or_else(|| self.used.map(|used| (limit - used).max(0.0)))?;
        Some((remaining / limit).clamp(0.0, 1.0) as f32)
    }

    /// Window duration in seconds, preferring the CLI field and falling back to
    /// well-known window ids/labels for older gateways.
    pub fn inferred_window_seconds(&self) -> Option<u64> {
        if let Some(seconds) = self.window_seconds.filter(|seconds| *seconds > 0) {
            return Some(seconds);
        }
        let id = self
            .quota_id
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let label = self.label.to_ascii_lowercase();
        if id.contains("five_hour")
            || id.contains("5h")
            || label == "5h"
            || label.ends_with(" 5h")
            || label.ends_with("· 5h")
        {
            Some(5 * 3_600)
        } else if id.contains("seven_day")
            || id.contains("weekly")
            || id.contains("7d")
            || label.contains("week")
            || label == "7d"
            || label.ends_with(" 7d")
            || label.ends_with("· 7d")
        {
            Some(7 * 86_400)
        } else if id.contains("monthly") || label.contains("month") {
            Some(30 * 86_400)
        } else {
            None
        }
    }

    /// Remaining fraction `0..=1` of the current reset window, used to place
    /// the pace tick mark on the remaining-quota progress bar.
    pub fn remaining_pace_fraction(&self) -> Option<f32> {
        if self.error.is_some() {
            return None;
        }
        let window_secs = self.inferred_window_seconds()? as f64;
        let reset = self
            .reset_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())?;
        let remaining_secs =
            (reset.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds() as f64;
        if remaining_secs < 0.0 || remaining_secs > window_secs {
            return None;
        }
        Some((remaining_secs / window_secs).clamp(0.0, 1.0) as f32)
    }

    pub fn is_percent_window(&self) -> bool {
        self.unit.as_deref() == Some("percent")
    }

    pub fn is_warning(&self) -> bool {
        if self.error.is_some() {
            return true;
        }
        let Some(remaining) = self.remaining_fraction() else {
            return false;
        };
        if remaining <= 0.20 {
            return true;
        }
        self.remaining_pace_fraction()
            .is_some_and(|pace| remaining + 0.15 < pace && remaining <= 0.75)
    }

    /// Remaining time until reset formatted compactly (`2h`, `3d 3h`, `45m`, `<1m`).
    pub fn reset_countdown(&self) -> Option<String> {
        let reset = self
            .reset_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())?;
        let seconds = (reset.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds();
        if seconds < 0 {
            return None;
        }
        if seconds < 60 {
            return Some("<1m".to_owned());
        }
        let minutes = seconds / 60;
        if minutes < 60 {
            return Some(format!("{minutes}m"));
        }
        let hours = minutes / 60;
        let rem_minutes = minutes % 60;
        if hours < 24 {
            if rem_minutes >= 5 && hours < 12 {
                return Some(format!("{hours}h {rem_minutes}m"));
            }
            return Some(format!("{hours}h"));
        }
        let days = hours / 24;
        let rem_hours = hours % 24;
        if rem_hours > 0 {
            Some(format!("{days}d {rem_hours}h"))
        } else {
            Some(format!("{days}d"))
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct UsageSeriesBucket {
    pub bucket_start_ms: i64,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub errors: u64,
    #[serde(default)]
    pub cached_requests: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub estimated_cost_usd: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct BenchmarkStatus {
    #[serde(default)]
    pub snapshot: Option<BenchmarkSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct BenchmarkSnapshot {
    pub run_id: String,
    pub status: String,
    #[serde(default)]
    pub total_models: usize,
    #[serde(default)]
    pub results: Vec<BenchmarkResult>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct BenchmarkResult {
    pub provider_id: String,
    pub upstream_model: String,
    pub status: String,
    #[serde(default)]
    pub ttft_ms: Option<f64>,
    #[serde(default)]
    pub tps: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn percent_entry(
        quota_id: Option<&str>,
        label: &str,
        used: Option<f64>,
        remaining: Option<f64>,
        window_seconds: Option<u64>,
        reset_in_secs: Option<i64>,
    ) -> QuotaEntry {
        QuotaEntry {
            quota_id: quota_id.map(str::to_owned),
            label: label.to_owned(),
            remaining,
            used,
            limit: Some(100.0),
            unit: Some("percent".to_owned()),
            currency: None,
            reset_at: reset_in_secs
                .map(|secs| (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339()),
            window_seconds,
            error: None,
        }
    }

    #[test]
    fn quota_remaining_fraction_and_warning_semantics() {
        let ok = percent_entry(Some("5h"), "5h", Some(37.0), Some(63.0), Some(18_000), None);
        assert!((ok.remaining_fraction().unwrap() - 0.63).abs() < 1e-4);
        assert!(!ok.is_warning());

        let warn = percent_entry(Some("week"), "Week", None, Some(15.0), None, None);
        assert!((warn.remaining_fraction().unwrap() - 0.15).abs() < 1e-4);
        assert!(warn.is_warning());

        // Derived from `used` when `remaining` is omitted.
        let from_used = percent_entry(Some("5h"), "5h", Some(85.0), None, Some(18_000), None);
        assert!((from_used.remaining_fraction().unwrap() - 0.15).abs() < 1e-4);
        assert!(from_used.is_warning());
    }

    #[test]
    fn quota_pace_fraction_uses_explicit_or_inferred_window() {
        // Explicit 5h window with 2.5h remaining => ~50% time remaining.
        let five_h = percent_entry(
            Some("5h"),
            "5h",
            Some(37.0),
            Some(63.0),
            Some(18_000),
            Some(9_000),
        );
        let pace = five_h
            .remaining_pace_fraction()
            .expect("pace should be computed");
        assert!((pace - 0.5).abs() < 0.02, "unexpected pace: {pace}");

        // Inferred weekly window from label with 3.5d remaining => ~50% time remaining.
        let weekly = percent_entry(None, "Weekly", Some(45.0), Some(55.0), None, Some(302_400));
        assert_eq!(weekly.inferred_window_seconds(), Some(604_800));
        let week_pace = weekly.remaining_pace_fraction().expect("weekly pace");
        assert!(
            (week_pace - 0.5).abs() < 0.02,
            "unexpected week pace: {week_pace}"
        );
    }
}
