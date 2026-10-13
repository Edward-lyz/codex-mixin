use super::engine::{Format, Patch, read_config};
use super::*;

fn patch(path: PathBuf, format: Format, fields: Vec<(Vec<String>, Value)>) -> Patch {
    Patch {
        path,
        format,
        fields,
    }
}

fn append_existing(path: &Path, keys: &[&str], additions: Vec<Value>) -> anyhow::Result<Value> {
    let doc = if !path.exists() && keys.is_empty() {
        json!([])
    } else {
        read_config(path, Format::Json)?
    };
    let mut cursor = &doc;
    for key in keys {
        cursor = cursor.get(*key).unwrap_or(&Value::Null);
    }
    let mut entries = if cursor.is_null() {
        Vec::new()
    } else {
        cursor
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("{}: model list must be an array", path.display()))?
    };
    anyhow::ensure!(
        !entries.iter().any(|v| v["vendor"] == PROVIDER
            || v["envKey"] == "CODEX_MIXIN_QWEN_API_KEY"
            || v["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("custom:codex-mixin/"))),
        "unmanaged codex-mixin model list collision"
    );
    entries.extend(additions);
    Ok(json!(entries))
}

pub(super) fn patches(
    id: &str,
    base: &str,
    key: &str,
    models: &[FrameworkModel],
) -> anyhow::Result<Vec<Patch>> {
    let first = &models[0];
    let v1 = format!("{base}/v1");
    Ok(match id {
        "hermes" => vec![patch(
            home_path("HERMES_HOME", ".hermes")?.join("config.yaml"),
            Format::Yaml,
            vec![
                field(
                    &["providers", PROVIDER],
                    json!({"name":PROVIDER,"base_url":v1,"api_key":key,"api_mode":"chat_completions","extra_headers":{"User-Agent":"hermes-agent"},"models":models.iter().map(|m|&m.id).collect::<Vec<_>>()}),
                ),
                field(&["model", "provider"], json!(PROVIDER)),
                field(&["model", "default"], json!(first.id)),
            ],
        )],
        "morph" => vec![patch(
            home_path("MISTER_MORPH_CONFIG", ".morph/config.yaml")?,
            Format::Yaml,
            vec![
                field(
                    &["llm", "inference_provider"],
                    json!("openai_response_compatible"),
                ),
                field(&["llm", "endpoint"], json!(v1)),
                field(&["llm", "api_key"], json!(key)),
                field(&["llm", "model"], json!(first.id)),
                field(
                    &["llm", "context_window_tokens"],
                    json!(first.context_window),
                ),
                field(&["llm", "headers", "User-Agent"], json!("mistermorph")),
            ],
        )],
        "qwen" => qwen(&v1, key, models)?,
        "droid" => droid(&v1, key, models)?,
        "qoder" | "qoder-cn" => {
            let (env, dir) = if id == "qoder" {
                ("QODER_CONFIG_DIR", ".qoder")
            } else {
                ("QODERCN_CONFIG_DIR", ".qoder-cn")
            };
            vec![patch(
                home_path(env, dir)?.join("settings.json"),
                Format::Json,
                vec![
                    field(
                        &["providers", PROVIDER],
                        json!({"displayName":PROVIDER,"protocol":"openai","baseUrl":v1,"apiKey":key,"model":first.id,
                    "models":models.iter().map(|m|json!({"model":m.id,"displayName":m.name,"contextWindow":m.context_window,"maxOutputTokens":m.max_tokens,"capabilities":{"tools":true,"vision":m.supports_image}})).collect::<Vec<_>>()}),
                    ),
                    field(
                        &["model", "name"],
                        json!(format!("{PROVIDER}/{}", first.id)),
                    ),
                ],
            )]
        }
        "omo" | "pencil" => pi_like(id, &v1, key, models)?,
        "crush" => crush(&v1, key, models)?,
        "mimocode" => mimocode(&v1, key, models)?,
        "goose" => goose(&v1, key, models)?,
        "omp" => omp(&v1, key, models)?,
        "kimi" | "grok" | "atomcode" => toml_agent(id, &v1, key, models)?,
        "copilot-jetbrains" => copilot_jb(&v1, key, models)?,
        "workbuddy" | "codebuddy" => buddy(id, &v1, key, models)?,
        "vscode" | "vscode-insiders" | "vscodium" => vscode(id, &v1, key, models)?,
        "zcode" => zcode(base, key, models)?,
        "air" => air(&v1, key, models)?,
        "openchamber" => openchamber(&v1, key, models)?,
        "t3code" => t3code(base, key, models)?,
        _ => anyhow::bail!("no source-verified configuration adapter for {id}"),
    })
}

fn qwen(v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let path = home_path("QWEN_HOME", ".qwen")?.join("settings.json");
    anyhow::ensure!(
        std::env::var_os("CODEX_MIXIN_QWEN_API_KEY").is_none(),
        "CODEX_MIXIN_QWEN_API_KEY in the shell overrides Qwen's managed key; unset it before connecting"
    );
    let entries = models.iter().map(|m|json!({"id":m.id,"name":format!("{PROVIDER}/{}",m.id),"baseUrl":v1,"envKey":"CODEX_MIXIN_QWEN_API_KEY","generationConfig":{"contextWindowSize":m.context_window}})).collect();
    let entries = append_existing(&path, &["modelProviders", "openai"], entries)?;
    Ok(vec![patch(
        path,
        Format::Json,
        vec![
            field(&["modelProviders", "openai"], entries),
            field(&["env", "CODEX_MIXIN_QWEN_API_KEY"], json!(key)),
            field(&["model", "name"], json!(models[0].id)),
            field(&["model", "baseUrl"], json!(v1)),
            field(&["security", "auth", "selectedType"], json!("openai")),
        ],
    )])
}

fn droid(v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let path = home_path("FACTORY_HOME_OVERRIDE", "")?.join(".factory/settings.json");
    let entries = models.iter().map(|m|json!({"id":format!("custom:{PROVIDER}/{}",m.id),"model":m.id,"displayName":m.name,"baseUrl":v1,"apiKey":key,"provider":"generic-chat-completion-api","maxContextLimit":m.context_window,"maxOutputTokens":m.max_tokens,"noImageSupport":!m.supports_image})).collect();
    let entries = append_existing(&path, &["customModels"], entries)?;
    Ok(vec![patch(
        path,
        Format::Json,
        vec![
            field(&["customModels"], entries),
            field(
                &["sessionDefaultSettings", "model"],
                json!(format!("custom:{PROVIDER}/{}", models[0].id)),
            ),
        ],
    )])
}

fn pi_like(id: &str, v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let dir = if id == "pencil" {
        home_path("", ".pencil")?
    } else if std::env::var_os("OMO_CODING_AGENT_DIR").is_some() {
        home_path("OMO_CODING_AGENT_DIR", ".omo/agent")?
    } else {
        home_path("SENPI_CODING_AGENT_DIR", ".omo/agent")?
    };
    let mut patches = vec![patch(
        dir.join("models.json"),
        Format::Json,
        vec![field(
            &["providers", PROVIDER],
            json!({"name":PROVIDER,"baseUrl":v1,"api":"openai-responses","apiKey":key,"models":pi_models(models,v1)}),
        )],
    )];
    if id == "omo" {
        patches.push(patch(
            dir.join("settings.json"),
            Format::Json,
            vec![
                field(&["defaultProvider"], json!(PROVIDER)),
                field(&["defaultModel"], json!(models[0].id)),
            ],
        ));
    }
    Ok(patches)
}

fn crush(v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let dir = if cfg!(target_os = "windows") {
        home_path("LOCALAPPDATA", "AppData/Local")?.join("crush")
    } else {
        config_home()?.join("crush")
    };
    let path = dir.join("crush.json");
    let data = if cfg!(target_os = "windows") {
        path.clone()
    } else {
        home_path("XDG_DATA_HOME", ".local/share")?.join("crush/crush.json")
    };
    let provider = field(
        &["providers", PROVIDER],
        json!({"type":"openai","name":PROVIDER,"base_url":v1,"api_key":key,"models":models.iter().map(|m|json!({"id":m.id,"name":m.name,"context_window":m.context_window,"default_max_tokens":m.max_tokens,"can_reason":!m.efforts.is_empty()})).collect::<Vec<_>>()}),
    );
    let picks = vec![
        field(
            &["models", "large"],
            json!({"provider":PROVIDER,"model":models[0].id}),
        ),
        field(
            &["models", "small"],
            json!({"provider":PROVIDER,"model":models[0].id}),
        ),
    ];
    if data == path {
        let mut fields = vec![provider];
        fields.extend(picks);
        return Ok(vec![patch(path, Format::Json, fields)]);
    }
    Ok(vec![
        patch(path, Format::Json, vec![provider]),
        patch(data, Format::Json, picks),
    ])
}

fn opencode_models(models: &[FrameworkModel]) -> Value {
    Value::Object(models.iter().map(|m|(m.id.clone(),json!({"name":m.name,"limit":{"context":m.context_window,"output":m.max_tokens},"attachment":m.supports_image,"reasoning":!m.efforts.is_empty(),"variants":Value::Object(m.efforts.iter().map(|e|(e.clone(),json!({"reasoningEffort":e}))).collect()),"modalities":{"input":if m.supports_image {vec!["text","image"]} else {vec!["text"]},"output":["text"]}}))).collect())
}

fn mimocode(v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let dir = if std::env::var_os("MIMOCODE_HOME").is_some() {
        home_path("MIMOCODE_HOME", "")?.join("config")
    } else {
        config_home()?.join("mimocode")
    };
    let path = ["mimocode.jsonc", "mimocode.json", "config.json"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|p| p.exists())
        .unwrap_or_else(|| dir.join("mimocode.json"));
    Ok(vec![patch(
        path,
        Format::Json,
        vec![
            field(
                &["provider", PROVIDER],
                json!({"npm":"@ai-sdk/openai-compatible","name":PROVIDER,"options":{"baseURL":v1,"apiKey":key},"models":opencode_models(models)}),
            ),
            field(&["model"], json!(format!("{PROVIDER}/{}", models[0].id))),
            field(
                &["small_model"],
                json!(format!("{PROVIDER}/{}", models[0].id)),
            ),
        ],
    )])
}

fn goose(v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let path = config_home()?.join("goose/config.yaml");
    let doc = read_config(&path, Format::Yaml)?;
    let fields = if doc.get("providers").is_some() || doc.get("active_provider").is_some() {
        vec![
            field(&["active_provider"], json!(PROVIDER)),
            field(
                &["providers", PROVIDER],
                json!({"enabled":true,"configured":true,"model":models[0].id}),
            ),
        ]
    } else {
        vec![
            field(&["GOOSE_PROVIDER"], json!(PROVIDER)),
            field(&["GOOSE_MODEL"], json!(models[0].id)),
        ]
    };
    let provider = json!({"name":PROVIDER,"engine":"openai","display_name":PROVIDER,"description":"Selected models through Codex Mixin","api_key_env":"","base_url":v1,"headers":{"Authorization":format!("Bearer {key}")},"requires_auth":false,"skip_canonical_filtering":true,"dynamic_models":false,"models":models.iter().map(|m|json!({"name":m.id,"context_limit":m.context_window,"reasoning":false})).collect::<Vec<_>>()});
    let custom = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("missing Goose config directory"))?
        .join("custom_providers/codex-mixin.json");
    anyhow::ensure!(
        !custom.exists(),
        "Goose provider file exists without a managed journal"
    );
    Ok(vec![
        patch(path, Format::Yaml, fields),
        patch(custom, Format::Json, vec![(vec![], provider)]),
    ])
}

fn omp(v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    anyhow::ensure!(
        std::env::var_os("PI_CODING_AGENT_DIR").is_none()
            && std::env::var_os("PI_CONFIG_DIR").is_none()
            && std::env::var_os("OMP_PROFILE").is_none()
            && std::env::var_os("PI_PROFILE").is_none(),
        "OMP profile or shared Pi directory requires explicit profile selection; unset its overrides for the default OMP adapter"
    );
    let dir = home_path("", ".omp/agent")?;
    let pick = |name: &str| {
        let yml = dir.join(format!("{name}.yml"));
        let yaml = dir.join(format!("{name}.yaml"));
        if !yml.exists() && yaml.exists() {
            yaml
        } else {
            yml
        }
    };
    Ok(vec![
        patch(
            pick("models"),
            Format::Yaml,
            vec![field(
                &["providers", PROVIDER],
                json!({"baseUrl":v1,"api":"openai-responses","apiKey":key,"headers":{"User-Agent":"omp"},"models":pi_models(models,v1)}),
            )],
        ),
        patch(
            pick("config"),
            Format::Yaml,
            vec![field(
                &["modelRoles", "default"],
                json!(format!("{PROVIDER}/{}", models[0].id)),
            )],
        ),
    ])
}

fn toml_agent(
    id: &str,
    v1: &str,
    key: &str,
    models: &[FrameworkModel],
) -> anyhow::Result<Vec<Patch>> {
    let (variable, default) = match id {
        "kimi" => ("KIMI_SHARE_DIR", ".kimi"),
        "grok" => ("GROK_HOME", ".grok"),
        _ => ("ATOMCODE_HOME", ".atomcode"),
    };
    let mut fields = Vec::new();
    let model_key = format!("{PROVIDER}/{}", models[0].id);
    match id {
        "kimi" => {
            fields.push(field(
                &["providers", PROVIDER],
                json!({"type":"kimi","base_url":v1,"api_key":key}),
            ));
            for m in models {
                fields.push(field(
                    &["models", &format!("{PROVIDER}/{}", m.id)],
                    json!({"provider":PROVIDER,"model":m.id,"max_context_size":m.context_window}),
                ));
            }
            fields.push(field(&["default_model"], json!(model_key)));
        }
        "grok" => {
            for m in models {
                fields.push(field(&["model",&format!("{PROVIDER}/{}",m.id)],json!({"model":m.id,"name":m.name,"base_url":v1,"api_key":key,"api_backend":"chat_completions","context_window":m.context_window})));
            }
            fields.push(field(&["models", "default"], json!(model_key)));
            fields.push(field(&["features", "campaigns"], json!(false)));
        }
        _ => {
            fields.push(field(
                &["provider_accounts", PROVIDER],
                json!({"provider":"openai-compatible","base_url":v1,"api_key":key}),
            ));
            for m in models {
                fields.push(field(&["models",&format!("{PROVIDER}/{}",m.id)],json!({"account":PROVIDER,"model":m.id,"context_window":m.context_window,"max_tokens":m.max_tokens,"supports_vision":m.supports_image})));
            }
            fields.push(field(&["default_model"], json!(model_key)));
            fields.push(field(&["default_provider"], json!(model_key)));
        }
    }
    let dir = if id == "kimi" {
        if std::env::var_os("KIMI_CODE_HOME").is_some() {
            home_path("KIMI_CODE_HOME", ".kimi-code")?
        } else if home_path("", ".kimi-code")?.exists() {
            home_path("", ".kimi-code")?
        } else if std::env::var_os("KIMI_SHARE_DIR").is_some() || home_path("", ".kimi")?.exists() {
            home_path("KIMI_SHARE_DIR", ".kimi")?
        } else {
            home_path("", ".kimi-code")?
        }
    } else {
        home_path(variable, default)?
    };
    Ok(vec![patch(dir.join("config.toml"), Format::Toml, fields)])
}

fn copilot_jb(v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let dir = if cfg!(target_os = "windows") && std::env::var_os("XDG_CONFIG_HOME").is_none() {
        home_path("", "AppData/Local")?
    } else {
        config_home()?
    };
    let entries = Value::Object(models.iter().map(|m|(m.id.clone(),json!({"deploymentUrl":format!("{v1}/chat/completions"),"isRegistered":true,"isCustomModel":true,"modelCapabilities":{"name":m.name,"maxInputTokens":m.context_window.saturating_sub(m.max_tokens),"maxOutputTokens":m.max_tokens,"toolCalling":true,"vision":m.supports_image}}))).collect());
    Ok(vec![patch(
        dir.join("github-copilot/byok.json"),
        Format::Json,
        vec![
            field(
                &["codex-mixin-provider-config"],
                json!({"groupName":PROVIDER,"apiType":"chatCompletions"}),
            ),
            field(&["codex-mixin-api-key"], json!(key)),
            field(&["codex-mixin-models-config"], entries),
        ],
    )])
}

fn buddy(id: &str, v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let (variable, default) = if id == "workbuddy" {
        ("WORKBUDDY_CONFIG_DIR", ".workbuddy")
    } else {
        ("CODEBUDDY_CONFIG_DIR", ".codebuddy")
    };
    let dir = home_path(variable, default)?;
    let path = dir.join("models.json");
    let doc = read_config(&path, Format::Json)?;
    let entries = models.iter().map(|m|json!({"id":m.id,"name":m.name,"vendor":PROVIDER,"apiKey":key,"url":format!("{v1}/chat/completions"),"maxInputTokens":m.context_window,"maxOutputTokens":m.max_tokens,"supportsToolCall":true,"supportsImages":m.supports_image,"supportsReasoning":!m.efforts.is_empty()})).collect();
    let root_array = doc.is_array() || (!path.exists() && id == "workbuddy");
    let fields = if root_array {
        vec![(vec![], append_existing(&path, &[], entries)?)]
    } else {
        vec![field(
            &["models"],
            append_existing(&path, &["models"], entries)?,
        )]
    };
    let mut out = vec![patch(path, Format::Json, fields)];
    if id == "codebuddy" {
        out.push(patch(
            dir.join("settings.json"),
            Format::Json,
            vec![field(&["model"], json!(models[0].id))],
        ));
    }
    Ok(out)
}

fn vscode(id: &str, v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    anyhow::ensure!(
        id != "vscodium",
        "VSCodium requires an installed Copilot Chat provider schema; automatic extension discovery is not implemented"
    );
    let folder = if id == "vscode-insiders" {
        "Code - Insiders"
    } else {
        "Code"
    };
    let root = if cfg!(target_os = "macos") {
        home_path("", "Library/Application Support")?
    } else if cfg!(target_os = "windows") {
        home_path("APPDATA", "AppData/Roaming")?
    } else {
        config_home()?
    };
    let dir = root.join(folder).join("User");
    let path = dir.join("chatLanguageModels.json");
    let doc = read_config(&path, Format::Json)?;
    let mut groups = if path.exists() {
        doc.as_array()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("VS Code model groups must be a root array"))?
    } else {
        Vec::new()
    };
    anyhow::ensure!(
        !groups.iter().any(|g| g["name"] == PROVIDER),
        "VS Code has an unmanaged codex-mixin model group"
    );
    groups.push(json!({"name":PROVIDER,"vendor":"customendpoint","apiType":"chat-completions","models":models.iter().map(|m|json!({"id":m.id,"name":m.name,"url":format!("{v1}/chat/completions"),"toolCalling":true,"vision":m.supports_image,"contextWindow":m.context_window,"maxOutputTokens":m.max_tokens.min(m.context_window/4),"requestHeaders":{"Authorization":format!("Bearer {key}")}})).collect::<Vec<_>>()}));
    Ok(vec![
        patch(path, Format::Json, vec![(vec![], json!(groups))]),
        patch(
            dir.join("settings.json"),
            Format::Json,
            vec![field(&["chat.defaultModel"], json!(models[0].id))],
        ),
    ])
}

fn t3code(base: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let path = home_path("T3CODE_HOME", ".t3")?.join("userdata/settings.json");
    let doc = read_config(&path, Format::Json)?;
    let mut config = json!({"customModels":models.iter().map(|m|json!({"slug":m.id,"name":m.name})).collect::<Vec<_>>()});
    for name in ["binaryPath", "homePath"] {
        let value = doc
            .pointer(&format!("/providerInstances/claudeAgent/config/{name}"))
            .or_else(|| doc.pointer(&format!("/providers/claudeAgent/{name}")));
        if let Some(value) = value {
            config[name] = value.clone();
        }
    }
    Ok(vec![patch(
        path,
        Format::Json,
        vec![field(
            &["providerInstances", PROVIDER],
            json!({"driver":"claudeAgent","displayName":PROVIDER,"enabled":true,"environment":[{"name":"ANTHROPIC_BASE_URL","value":base,"sensitive":false},{"name":"ANTHROPIC_AUTH_TOKEN","value":key,"sensitive":true}],"config":config}),
        )],
    )])
}

fn open_code_provider(v1: &str, key: &str, models: &[FrameworkModel]) -> Value {
    json!({"npm":"@ai-sdk/openai-compatible","name":PROVIDER,"options":{"baseURL":v1,"apiKey":key},"models":opencode_models(models)})
}

fn air(v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let root = if cfg!(target_os = "macos") {
        home_path("", "Library/Application Support")?
    } else if cfg!(target_os = "windows") {
        home_path("APPDATA", "AppData/Roaming")?
    } else {
        config_home()?
    };
    let dir = root.join("JetBrains/Air");
    let config = dir.join("codex-mixin-opencode.json");
    let binary = crate::platform::agent_cli::find_command("opencode")
        .or_else(|| {
            let name = if cfg!(target_os = "windows") {
                "opencode.exe"
            } else {
                "opencode"
            };
            let binary = crate::platform::home_dir().join(".opencode/bin").join(name);
            binary.is_file().then_some(binary)
        })
        .ok_or_else(|| anyhow::anyhow!("Air requires an installed OpenCode ACP server"))?;
    let model = format!("{PROVIDER}/{}", models[0].id);
    Ok(vec![
        patch(
            config.clone(),
            Format::Json,
            vec![
                field(&["provider", PROVIDER], open_code_provider(v1, key, models)),
                field(&["enabled_providers"], json!([PROVIDER])),
                field(&["model"], json!(model)),
                field(&["small_model"], json!(model)),
            ],
        ),
        patch(
            dir.join("acp.json"),
            Format::Json,
            vec![field(
                &["agent_servers", "Codex Mixin"],
                json!({"command":binary,"args":["acp"],"env":{"OPENCODE_CONFIG":config}}),
            )],
        ),
    ])
}

fn openchamber(v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let dir = home_path("OPENCHAMBER_DATA_DIR", ".config/openchamber")?;
    let oc_dir = if std::env::var_os("OPENCODE_CONFIG_DIR").is_some() {
        home_path("OPENCODE_CONFIG_DIR", "")?
    } else {
        config_home()?.join("opencode")
    };
    let oc_path = if std::env::var_os("OPENCODE_CONFIG").is_some() {
        home_path("OPENCODE_CONFIG", "")?
    } else {
        ["opencode.jsonc", "opencode.json", "config.json"]
            .into_iter()
            .map(|s| oc_dir.join(s))
            .find(|p| p.exists())
            .unwrap_or_else(|| oc_dir.join("opencode.json"))
    };
    // An independent provider avoids taking ownership of the OpenCode row.
    let provider_id = "codex-mixin-openchamber";
    let model = format!("{provider_id}/{}", models[0].id);
    let mut out = vec![patch(
        oc_path,
        Format::Json,
        vec![field(
            &["provider", provider_id],
            open_code_provider(v1, key, models),
        )],
    )];
    let settings = dir.join("settings.json");
    let doc = read_config(&settings, Format::Json)?;
    let mut fields = vec![
        field(&["defaultModel"], json!(model)),
        field(&["smallModelUseDefault"], json!(false)),
        field(&["smallModelOverride"], json!(model)),
    ];
    if let Some(projects) = doc.get("projects") {
        let projects = projects
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("OpenChamber projects must be an array"))?;
        anyhow::ensure!(
            !projects.iter().any(|p| p.get("defaultModel").is_some()),
            "OpenChamber project defaultModel overrides its profile; select the Mixin model per project or remove the override before connecting"
        );
    }
    let preferences = dir.join("preferences.json");
    if preferences.exists() {
        let prefs = read_config(&preferences, Format::Json)?;
        anyhow::ensure!(
            prefs["version"] == 1 && prefs["fields"].is_object(),
            "OpenChamber preferences must have version=1 and a fields object"
        );
        let now = chrono::Utc::now().timestamp_millis();
        let profile_fields = fields
            .iter()
            .map(|(path, value)| {
                (
                    vec!["fields".to_owned(), path[0].clone()],
                    json!({"value":value,"updatedAt":now}),
                )
            })
            .collect();
        out.push(patch(preferences, Format::Json, profile_fields));
        if !settings.exists() {
            fields.clear();
        }
    }
    if !fields.is_empty() {
        out.push(patch(settings, Format::Json, fields));
    }
    Ok(out)
}

fn zcode(base: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let dir = home_path("", ".zcode/v2")?;
    let rules = dir.join("provider_config.json");
    let doc = read_config(&rules, Format::Json)?;
    let read_rules = |pointer: &str| -> anyhow::Result<Vec<Value>> {
        let Some(value) = doc.pointer(pointer) else {
            return Ok(Vec::new());
        };
        let values = value
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("ZCode {pointer} must be an array"))?;
        anyhow::ensure!(
            !values.iter().any(|v| v["providerId"] == PROVIDER),
            "ZCode has unmanaged codex-mixin rules"
        );
        Ok(values)
    };
    let mut provider_rules = read_rules("/config/providerConfigRules/providerRules")?;
    let mut model_rules = read_rules("/config/modelConfigRules/providerModelRules")?;
    let ids: Vec<_> = models.iter().map(|m| &m.id).collect();
    provider_rules.push(json!({"providerId":PROVIDER,"providerName":PROVIDER,"enabled":true,"config":{"group":"standard-personal","access":{"type":"api-key","apiKey":key},"api":{"type":"anthropic-messages","baseUrl":base},"personalModelIds":ids,"modelOrder":ids}}));
    for model in models {
        model_rules.push(json!({"providerId":PROVIDER,"modelId":model.id,"config":{"properties":{"contextWindow":model.context_window,"inputFormat":{"supportsImage":model.supports_image}},"optionSpecs":{"maxOutputTokens":{"max":model.max_tokens.min(128000)}}}}));
    }
    let mut fields = vec![
        field(
            &["config", "providerConfigRules", "providerRules"],
            json!(provider_rules),
        ),
        field(
            &["config", "modelConfigRules", "providerModelRules"],
            json!(model_rules),
        ),
    ];
    if doc.get("schemaVersion").is_none() {
        fields.push(field(&["schemaVersion"], json!(1)));
    } else {
        anyhow::ensure!(
            doc["schemaVersion"] == 1,
            "unsupported ZCode rule schema version"
        );
    }
    Ok(vec![
        patch(
            dir.join("config.json"),
            Format::Json,
            vec![field(
                &["provider", PROVIDER],
                json!({"name":PROVIDER,"kind":"anthropic","enabled":true,"source":"custom","options":{"apiKey":key,"baseURL":base},"models":opencode_models(models)}),
            )],
        ),
        patch(rules, Format::Json, fields),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pi_and_opencode_models_keep_literal_model_ids() {
        let models = vec![FrameworkModel {
            id: "provider/model.with.dots".to_owned(),
            name: "Model".to_owned(),
            context_window: 128000,
            max_tokens: 8192,
            supports_image: true,
            protocol: crate::provider::ProviderProtocol::OpenAiChat,
            efforts: vec!["high".to_owned()],
        }];
        assert_eq!(
            opencode_models(&models)["provider/model.with.dots"]["limit"]["context"],
            128000
        );
        assert_eq!(
            pi_models(&models, "http://127.0.0.1:8787/v1")[0]["input"],
            json!(["text", "image"])
        );
    }
    #[test]
    fn array_append_preserves_existing_entries_and_rejects_collisions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("models.json");
        std::fs::write(&path, "[{\"id\":\"own\"}]").unwrap();
        let appended = append_existing(&path, &[], vec![json!({"id":"managed"})]).unwrap();
        assert_eq!(appended[0]["id"], "own");
        assert_eq!(appended[1]["id"], "managed");
        std::fs::write(&path, "[{\"vendor\":\"codex-mixin\"}]").unwrap();
        assert!(append_existing(&path, &[], Vec::new()).is_err());
    }
}
