use super::*;

use std::collections::HashSet;

use codex_mixin::provider::AWS_BEDROCK_DEFAULT_REGION;
use serde_json::Value;

impl AddProviderForm {
    pub(super) fn new(providers: &[Value]) -> Self {
        Self {
            aws_region: AWS_BEDROCK_DEFAULT_REGION.to_owned(),
            existing_ids: providers
                .iter()
                .filter_map(|provider| provider.get("id"))
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            ..Self::default()
        }
    }

    pub(super) fn preset(&self) -> &'static str {
        PROVIDER_PRESETS[self.preset_index]
    }

    pub(super) fn active_fields(&self) -> &'static [usize] {
        match self.preset() {
            "custom" => &[0, 1, 2, 3, 4, 5, 12, 11],
            "baidu-oneapi" => &[0, 1, 5, 6, 9, 10, 12, 11],
            "opencode-go" => &[0, 1, 5, 7, 8, 12, 11],
            "aws-bedrock" => &[0, 1, 13, 14, 15, 16, 12, 11],
            _ => &[0, 1, 5, 12, 11],
        }
    }

    pub(super) fn provider_id(&self) -> String {
        if !self.id.trim().is_empty() {
            return self.id.trim().to_owned();
        }
        let base = self.preset();
        if !self.existing_ids.contains(base) {
            return base.to_owned();
        }
        let mut suffix = 2_u64;
        loop {
            let candidate = format!("{base}-{suffix}");
            if !self.existing_ids.contains(&candidate) {
                return candidate;
            }
            suffix = suffix.saturating_add(1);
        }
    }

    pub(super) fn move_focus(&mut self, offset: isize) {
        let fields = self.active_fields();
        let position = fields
            .iter()
            .position(|field| *field == self.focus)
            .unwrap_or(0);
        self.focus =
            fields[(position as isize + offset).rem_euclid(fields.len() as isize) as usize];
    }

    pub(super) fn focused_text(&mut self) -> Option<&mut String> {
        match self.focus {
            1 => Some(&mut self.id),
            2 => Some(&mut self.display_name),
            3 => Some(&mut self.base_url),
            4 => Some(&mut self.website_url),
            5 => Some(&mut self.api_key),
            6 => Some(&mut self.quota_username),
            7 => Some(&mut self.quota_workspace_id),
            8 => Some(&mut self.quota_auth_cookie),
            12 => Some(&mut self.image_generation_path),
            13 => Some(&mut self.aws_region),
            14 => Some(&mut self.aws_access_key_id),
            15 => Some(&mut self.aws_secret_access_key),
            16 => Some(&mut self.aws_session_token),
            _ => None,
        }
    }

    pub(super) fn toggle_focused(&mut self, offset: isize) {
        match self.focus {
            0 => {
                self.preset_index = (self.preset_index as isize + offset)
                    .clamp(0, PROVIDER_PRESETS.len().saturating_sub(1) as isize)
                    as usize;
                if is_aws_bedrock(self.preset()) && self.aws_region.trim().is_empty() {
                    self.aws_region = AWS_BEDROCK_DEFAULT_REGION.to_owned();
                }
                if !self.active_fields().contains(&self.focus) {
                    self.focus = self.active_fields()[0];
                }
            }
            9 => {
                self.baidu_auth_bridge =
                    (self.baidu_auth_bridge as isize + offset).clamp(0, 1) as usize;
            }
            10 => self.baidu_code_report = !self.baidu_code_report,
            11 => self.auxiliary_model_upstream = !self.auxiliary_model_upstream,
            _ => {}
        }
    }

    pub(super) fn baidu_auth_bridge_name(&self) -> &'static str {
        if self.baidu_auth_bridge == 0 {
            "Disabled"
        } else {
            "DUCX loopback"
        }
    }

    pub(super) fn args(&self) -> anyhow::Result<Vec<String>> {
        let preset = self.preset();
        if is_aws_bedrock(preset) {
            anyhow::ensure!(!self.aws_region.trim().is_empty(), "AWS region is required");
            anyhow::ensure!(
                !self.aws_access_key_id.trim().is_empty(),
                "AWS access key ID is required"
            );
            anyhow::ensure!(
                !self.aws_secret_access_key.trim().is_empty(),
                "AWS secret access key is required"
            );
        } else {
            anyhow::ensure!(!self.api_key.trim().is_empty(), "API key is required");
        }
        if preset == "custom" {
            anyhow::ensure!(
                !self.display_name.trim().is_empty(),
                "display name is required"
            );
            anyhow::ensure!(!self.base_url.trim().is_empty(), "base URL is required");
        }
        if preset == "baidu-oneapi" {
            anyhow::ensure!(
                !self.quota_username.trim().is_empty(),
                "quota username is required"
            );
        }
        if preset == "opencode-go" {
            anyhow::ensure!(
                !self.quota_workspace_id.trim().is_empty()
                    && !self.quota_auth_cookie.trim().is_empty(),
                "workspace ID and auth cookie are required"
            );
        }
        let mut args = vec![
            "providers".to_owned(),
            "add".to_owned(),
            "--preset".to_owned(),
            preset.to_owned(),
        ];
        if is_aws_bedrock(preset) {
            args.extend([
                "--aws-access-key-id".to_owned(),
                self.aws_access_key_id.trim().to_owned(),
                "--aws-secret-access-key".to_owned(),
                self.aws_secret_access_key.trim().to_owned(),
                "--aws-region".to_owned(),
                self.aws_region.trim().to_owned(),
            ]);
            if !self.aws_session_token.trim().is_empty() {
                args.extend([
                    "--aws-session-token".to_owned(),
                    self.aws_session_token.trim().to_owned(),
                ]);
            }
        } else {
            args.extend(["--key".to_owned(), self.api_key.trim().to_owned()]);
        }
        let provider_id = self.provider_id();
        args.extend(["--id".to_owned(), provider_id]);
        let mut optional = Vec::new();
        if preset == "custom" {
            optional.extend([
                ("--display-name", self.display_name.as_str()),
                ("--base-url", self.base_url.as_str()),
                ("--website-url", self.website_url.as_str()),
            ]);
        } else if preset == "baidu-oneapi" {
            optional.push(("--quota-username", self.quota_username.as_str()));
        } else if preset == "opencode-go" {
            optional.extend([
                ("--quota-workspace-id", self.quota_workspace_id.as_str()),
                ("--quota-auth-cookie", self.quota_auth_cookie.as_str()),
            ]);
        }
        for (flag, value) in optional {
            if !value.trim().is_empty() {
                args.extend([flag.to_owned(), value.trim().to_owned()]);
            }
        }
        if !self.image_generation_path.trim().is_empty() {
            args.extend([
                "--image-generation-path".to_owned(),
                self.image_generation_path.trim().to_owned(),
            ]);
        }
        args.extend([
            "--auxiliary-model-upstream".to_owned(),
            self.auxiliary_model_upstream.to_string(),
        ]);
        if preset == "baidu-oneapi" {
            args.extend([
                "--baidu-auth-bridge".to_owned(),
                if self.baidu_auth_bridge == 0 {
                    "disabled"
                } else {
                    "ducx_loopback"
                }
                .to_owned(),
                "--baidu-code-report".to_owned(),
                self.baidu_code_report.to_string(),
            ]);
        }
        Ok(args)
    }

    pub(super) fn clear_secrets(&mut self) {
        self.api_key.clear();
        self.quota_auth_cookie.clear();
        self.aws_access_key_id.clear();
        self.aws_secret_access_key.clear();
        self.aws_session_token.clear();
    }
}

impl SetupForm {
    pub(super) fn new(providers: &[Value]) -> Self {
        Self {
            provider: AddProviderForm::new(providers),
            focus: 0,
            codex_mode: 0,
        }
    }

    pub(super) fn active_fields(&self) -> Vec<usize> {
        let mut fields = self.provider.active_fields().to_vec();
        fields.push(17);
        fields
    }

    pub(super) fn move_focus(&mut self, offset: isize) {
        let fields = self.active_fields();
        let position = fields
            .iter()
            .position(|field| *field == self.focus)
            .unwrap_or(0);
        self.focus =
            fields[(position as isize + offset).rem_euclid(fields.len() as isize) as usize];
        if self.focus != 17 {
            self.provider.focus = self.focus;
        }
    }

    pub(super) fn codex_mode_name(&self) -> &'static str {
        match self.codex_mode {
            0 => "Official account",
            1 => "Custom models only",
            _ => "Skip for now",
        }
    }

    pub(super) fn reset(&mut self, providers: &[Value]) {
        *self = Self::new(providers);
    }
}

impl EditProviderForm {
    pub(super) fn from_provider(provider: &Value) -> Option<Self> {
        (provider.get("kind").and_then(Value::as_str) == Some("configured")).then(|| Self {
            focus: 0,
            id: value_str(provider, "id", "-").to_owned(),
            preset: value_str(provider, "preset_id", "custom").to_owned(),
            enabled: provider
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            display_name: value_str(provider, "display_name", "").to_owned(),
            base_url: value_str(provider, "base_url", "").to_owned(),
            website_url: value_str(provider, "website_url", "").to_owned(),
            website_url_configured: provider
                .get("website_url")
                .is_some_and(|value| !value.is_null()),
            image_generation_path: value_str(provider, "image_generation_path", "").to_owned(),
            image_generation_configured: provider
                .get("image_generation_path")
                .is_some_and(|value| !value.is_null()),
            api_key: String::new(),
            api_key_configured: provider
                .get("api_key_configured")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            clear_key: false,
            aws_access_key_id: String::new(),
            aws_secret_access_key: String::new(),
            aws_session_token: String::new(),
            aws_region: value_str(provider, "aws_region", AWS_BEDROCK_DEFAULT_REGION).to_owned(),
            aws_sigv4_configured: provider
                .get("aws_sigv4_configured")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            aws_session_token_configured: provider
                .get("aws_session_token_configured")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            clear_aws_session_token: false,
            clear_aws_credentials: false,
            quota_username: value_str(provider, "quota_username", "").to_owned(),
            quota_workspace_id: value_str(provider, "quota_workspace_id", "").to_owned(),
            original_quota_workspace_id: value_str(provider, "quota_workspace_id", "").to_owned(),
            quota_auth_cookie: String::new(),
            quota_auth_cookie_configured: provider
                .get("quota_auth_cookie_configured")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            clear_quota: false,
            baidu_auth_bridge: usize::from(
                value_str(provider, "baidu_auth_bridge", "disabled") == "ducx_loopback",
            ),
            baidu_code_report: provider
                .get("baidu_code_report")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            auxiliary_model_upstream: provider
                .get("auxiliary_model_upstream")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    pub(super) fn active_fields(&self) -> &'static [usize] {
        match self.preset.as_str() {
            "custom" => &[0, 1, 2, 3, 4, 5, 9],
            "baidu-oneapi" => &[3, 4, 5, 6, 7, 8, 9],
            "opencode-go" => &[3, 4, 5, 10, 11, 12, 9],
            "aws-bedrock" => &[13, 14, 15, 16, 17, 18, 3, 9],
            _ => &[3, 4, 5, 9],
        }
    }

    pub(super) fn move_focus(&mut self, offset: isize) {
        let fields = self.active_fields();
        let position = fields
            .iter()
            .position(|field| *field == self.focus)
            .unwrap_or(0);
        self.focus =
            fields[(position as isize + offset).rem_euclid(fields.len() as isize) as usize];
    }

    pub(super) fn focused_text(&mut self) -> Option<&mut String> {
        match self.focus {
            0 => Some(&mut self.display_name),
            1 => Some(&mut self.base_url),
            2 => Some(&mut self.website_url),
            3 => Some(&mut self.image_generation_path),
            4 => Some(&mut self.api_key),
            6 => Some(&mut self.quota_username),
            10 => Some(&mut self.quota_workspace_id),
            11 => Some(&mut self.quota_auth_cookie),
            13 => Some(&mut self.aws_region),
            14 => Some(&mut self.aws_access_key_id),
            15 => Some(&mut self.aws_secret_access_key),
            16 => Some(&mut self.aws_session_token),
            _ => None,
        }
    }

    pub(super) fn toggle_focused(&mut self, offset: isize) {
        match self.focus {
            5 if self.api_key_configured => {
                self.clear_key = !self.clear_key;
                if self.clear_key {
                    self.api_key.clear();
                }
            }
            7 => {
                self.baidu_auth_bridge =
                    (self.baidu_auth_bridge as isize + offset).clamp(0, 1) as usize;
            }
            8 => self.baidu_code_report = !self.baidu_code_report,
            9 => self.auxiliary_model_upstream = !self.auxiliary_model_upstream,
            12 if self.quota_auth_cookie_configured => {
                self.clear_quota = !self.clear_quota;
                if self.clear_quota {
                    self.quota_auth_cookie.clear();
                    self.quota_workspace_id.clear();
                }
            }
            17 if self.aws_session_token_configured => {
                self.clear_aws_session_token = !self.clear_aws_session_token;
                if self.clear_aws_session_token {
                    self.aws_session_token.clear();
                }
            }
            18 if self.aws_sigv4_configured => {
                self.clear_aws_credentials = !self.clear_aws_credentials;
                if self.clear_aws_credentials {
                    self.aws_access_key_id.clear();
                    self.aws_secret_access_key.clear();
                    self.aws_session_token.clear();
                    self.clear_aws_session_token = false;
                }
            }
            _ => {}
        }
    }

    pub(super) fn args(&self) -> anyhow::Result<Vec<String>> {
        let mut args = vec!["providers".to_owned(), "update".to_owned(), self.id.clone()];
        if self.preset == "custom" {
            anyhow::ensure!(
                !self.display_name.trim().is_empty(),
                "display name is required"
            );
            anyhow::ensure!(!self.base_url.trim().is_empty(), "base URL is required");
            args.extend([
                "--display-name".to_owned(),
                self.display_name.trim().to_owned(),
                "--base-url".to_owned(),
                self.base_url.trim().to_owned(),
            ]);
            if !self.website_url.trim().is_empty() || self.website_url_configured {
                args.extend([
                    "--website-url".to_owned(),
                    self.website_url.trim().to_owned(),
                ]);
            }
        }
        if self.clear_key {
            anyhow::ensure!(
                !self.enabled,
                "disable the provider before clearing its API key"
            );
            args.push("--clear-key".to_owned());
        } else if !self.api_key.trim().is_empty() {
            args.extend(["--key".to_owned(), self.api_key.trim().to_owned()]);
        }
        if is_aws_bedrock(&self.preset) {
            anyhow::ensure!(!self.aws_region.trim().is_empty(), "AWS region is required");
            if self.clear_aws_credentials {
                anyhow::ensure!(
                    !self.enabled,
                    "disable the provider before clearing its AWS credentials"
                );
                args.push("--clear-aws-credentials".to_owned());
            } else {
                args.extend(["--aws-region".to_owned(), self.aws_region.trim().to_owned()]);
                if !self.aws_access_key_id.trim().is_empty() {
                    args.extend([
                        "--aws-access-key-id".to_owned(),
                        self.aws_access_key_id.trim().to_owned(),
                    ]);
                }
                if !self.aws_secret_access_key.trim().is_empty() {
                    args.extend([
                        "--aws-secret-access-key".to_owned(),
                        self.aws_secret_access_key.trim().to_owned(),
                    ]);
                }
                if self.clear_aws_session_token {
                    args.push("--clear-aws-session-token".to_owned());
                } else if !self.aws_session_token.trim().is_empty() {
                    args.extend([
                        "--aws-session-token".to_owned(),
                        self.aws_session_token.trim().to_owned(),
                    ]);
                }
            }
        }
        if self.image_generation_path.trim().is_empty() {
            if self.image_generation_configured {
                args.push("--clear-image-generation".to_owned());
            }
        } else {
            args.extend([
                "--image-generation-path".to_owned(),
                self.image_generation_path.trim().to_owned(),
            ]);
        }
        args.extend([
            "--auxiliary-model-upstream".to_owned(),
            self.auxiliary_model_upstream.to_string(),
        ]);
        if self.preset == "baidu-oneapi" {
            anyhow::ensure!(
                !self.quota_username.trim().is_empty(),
                "quota username is required"
            );
            args.extend([
                "--quota-username".to_owned(),
                self.quota_username.trim().to_owned(),
                "--baidu-auth-bridge".to_owned(),
                if self.baidu_auth_bridge == 0 {
                    "disabled"
                } else {
                    "ducx_loopback"
                }
                .to_owned(),
                "--baidu-code-report".to_owned(),
                self.baidu_code_report.to_string(),
            ]);
        }
        if self.preset == "opencode-go" {
            if self.clear_quota {
                args.push("--clear-quota".to_owned());
            } else if self.quota_workspace_id.trim() != self.original_quota_workspace_id
                || !self.quota_auth_cookie.trim().is_empty()
            {
                anyhow::ensure!(
                    !self.quota_workspace_id.trim().is_empty()
                        && !self.quota_auth_cookie.trim().is_empty(),
                    "workspace ID and auth cookie must be entered together"
                );
                args.extend([
                    "--quota-workspace-id".to_owned(),
                    self.quota_workspace_id.trim().to_owned(),
                    "--quota-auth-cookie".to_owned(),
                    self.quota_auth_cookie.trim().to_owned(),
                ]);
            }
        }
        Ok(args)
    }
}

impl FusionForm {
    pub(super) fn new(snapshot: &Snapshot) -> Self {
        let available = snapshot
            .models
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .filter(|id| !id.starts_with("mixin/fusion/"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let profile = snapshot.fusion_profile.as_ref();
        let loaded_profile_id = profile
            .and_then(|profile| profile.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let stored_panels = profile
            .and_then(|profile| profile.get("panel_models"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|model| available.iter().any(|available| available == model))
            .map(str::to_owned)
            .take(8)
            .collect::<HashSet<_>>();
        let panel_models = if stored_panels.is_empty() {
            available.iter().take(3).cloned().collect()
        } else {
            stored_panels
        };
        let stored_judge = profile
            .and_then(|profile| profile.get("judge_model"))
            .and_then(Value::as_str)
            .filter(|model| available.iter().any(|available| available == model));
        let judge_model = stored_judge
            .or_else(|| available.first().map(String::as_str))
            .unwrap_or_default()
            .to_owned();
        let stored_final = profile
            .and_then(|profile| profile.get("final_model"))
            .and_then(Value::as_str)
            .filter(|model| available.iter().any(|available| available == model));
        let final_model = stored_final
            .or_else(|| {
                available
                    .get(1)
                    .or_else(|| available.first())
                    .map(String::as_str)
            })
            .unwrap_or_default()
            .to_owned();
        Self {
            profile_id: loaded_profile_id
                .clone()
                .unwrap_or_else(|| "default".to_owned()),
            loaded_profile_id,
            panel_models,
            model_index: 0,
            judge_model,
            final_model,
            min_successful: profile
                .and_then(|profile| profile.get("min_successful"))
                .and_then(Value::as_u64)
                .unwrap_or(1) as usize,
            timeout_ms: profile
                .and_then(|profile| profile.get("timeout_ms"))
                .and_then(Value::as_u64)
                .unwrap_or(300_000),
            show_intermediate_results: profile
                .and_then(|profile| profile.get("show_intermediate_results"))
                .and_then(Value::as_bool)
                .unwrap_or(true),
            panel_tools_enabled: profile
                .and_then(|profile| profile.get("panel_tools"))
                .and_then(|tools| tools.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(true),
            editing_profile_id: false,
        }
    }

    pub(super) fn selected_model<'a>(&self, models: &'a [Value]) -> Option<&'a str> {
        fusion_models(models)
            .get(self.model_index)
            .and_then(|model| model.get("id"))
            .and_then(Value::as_str)
    }

    pub(super) fn args(&self, models: &[Value]) -> anyhow::Result<Vec<String>> {
        let id = self.profile_id.trim();
        anyhow::ensure!(
            !id.is_empty() && !id.contains('/'),
            "invalid Fusion profile ID"
        );
        anyhow::ensure!(
            (1..=8).contains(&self.panel_models.len()),
            "select between 1 and 8 Panel models"
        );
        anyhow::ensure!(
            !self.judge_model.is_empty() && !self.final_model.is_empty(),
            "select Judge and Final models"
        );
        anyhow::ensure!(
            (1..=self.panel_models.len()).contains(&self.min_successful),
            "minimum successful Panels must not exceed the Panel count"
        );
        let ordered_panels = models
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .filter(|model| self.panel_models.contains(*model))
            .collect::<Vec<_>>();
        let profile = serde_json::json!({
            "id": id,
            "panel_models": ordered_panels,
            "judge_model": self.judge_model,
            "final_model": self.final_model,
            "min_successful": self.min_successful,
            "max_completion_tokens": 2048,
            "timeout_ms": self.timeout_ms,
            "show_intermediate_results": self.show_intermediate_results,
            "panel_tools": {
                "enabled": self.panel_tools_enabled,
                "max_rounds": 16,
                "max_calls_per_model": 64
            }
        });
        let mut args = vec![
            "fusion".to_owned(),
            "set".to_owned(),
            "--profile-json".to_owned(),
            serde_json::to_string(&profile)?,
        ];
        if let Some(loaded_id) = &self.loaded_profile_id {
            args.extend(["--replace-id".to_owned(), loaded_id.clone()]);
        }
        Ok(args)
    }
}
