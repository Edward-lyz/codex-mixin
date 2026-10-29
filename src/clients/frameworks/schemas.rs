use serde_json::Map;

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
        "gemini" => gemini_at(&home_path("", ".gemini")?, base, key, models),
        "agy" => agy_at(&home_path("", ".gemini/antigravity-cli")?, models),
        "muse" => muse_at(&config_home()?.join("muse"), &v1, first)?,
        "cursor-local" => Vec::new(),
        "reasonix" => reasonix(&crate::platform::reasonix_home()?, &v1, key, models)?,
        "minimax-code" => {
            let dir = if std::env::var_os("MINIMAX_DATA_DIR").is_some() {
                home_path("MINIMAX_DATA_DIR", ".minimax")?
            } else {
                home_path("MAVIS_DATA_DIR", ".minimax")?
            };
            minimax(&dir, &v1, key, models)
        }
        "cline" => cline(&v1, key, models)?,
        "commandcode" | "fx" | "empryo" | "ante" => keyless(id, &v1, models)?,
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

fn gemini_at(dir: &Path, base: &str, key: &str, models: &[FrameworkModel]) -> Vec<Patch> {
    let mut settings = vec![
        field(
            &["security", "auth", "selectedType"],
            json!("gemini-api-key"),
        ),
        field(&["model", "name"], json!(models[0].id)),
        field(&["experimental", "dynamicModelConfiguration"], json!(true)),
    ];
    for model in models {
        settings.push(field(
            &["modelConfigs", "modelDefinitions", &model.id],
            json!({"displayName":format!("Codex Mixin: {}", model.name),
                "tier":"custom","family":"custom","isVisible":true,
                "features":{"thinking":!model.efforts.is_empty()}}),
        ));
        settings.push(field(
            &["modelConfigs", "customAliases", &model.id],
            json!({"modelConfig":{"model":model.id,
                "generateContentConfig":{"maxOutputTokens":model.max_tokens}}}),
        ));
    }
    vec![
        patch(dir.join("settings.json"), Format::Json, settings),
        patch(
            dir.join(".env"),
            Format::Env,
            vec![
                field(&["GOOGLE_GEMINI_BASE_URL"], json!(base)),
                field(&["GEMINI_API_KEY"], json!(key)),
            ],
        ),
    ]
}

fn agy_at(dir: &Path, models: &[FrameworkModel]) -> Vec<Patch> {
    let mut fields = vec![
        field(&["modelProvider"], json!("gemini")),
        field(&["model"], json!(format!("{PROVIDER}/{}", models[0].id))),
    ];
    fields.extend(models.iter().map(|model| {
        field(
            &[
                "customModelsConfig",
                "customModels",
                &format!("{PROVIDER}/{}", model.id),
            ],
            json!({"apiProvider":"gemini","modelName":model.id}),
        )
    }));
    vec![patch(dir.join("settings.json"), Format::Json, fields)]
}

fn muse_at(dir: &Path, v1: &str, model: &FrameworkModel) -> anyhow::Result<Vec<Patch>> {
    let path = dir.join("settings.json");
    let document = read_config(&path, Format::Json)?;
    anyhow::ensure!(
        document
            .get("schema_version")
            .is_none_or(|version| version == 1),
        "unsupported Muse settings schema_version"
    );
    Ok(vec![patch(
        path,
        Format::Json,
        vec![
            field(&["schema_version"], json!(1)),
            field(
                &["endpoint_transport"],
                json!({
                    "base_url":v1,
                    "auth":if cfg!(feature="client-auth") {"bearer"} else {"none"}
                }),
            ),
            field(&["model"], json!(model.id)),
        ],
    )])
}

fn reasonix(
    dir: &Path,
    v1: &str,
    key: &str,
    models: &[FrameworkModel],
) -> anyhow::Result<Vec<Patch>> {
    let path = dir.join("config.toml");
    let config = read_config(&path, Format::Toml)?;
    let mut providers = match config.get("providers") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Reasonix providers must be an array"))?,
    };
    anyhow::ensure!(
        !providers.iter().any(|v| v["name"] == PROVIDER),
        "unmanaged Reasonix provider collision"
    );
    let overrides: Map<String, Value> = models.iter().map(|m| (m.id.clone(), json!({"context_window":m.context_window,"max_output_tokens":m.max_tokens,"vision":m.supports_image}))).collect();
    providers.push(json!({"name":PROVIDER,"kind":"openai","base_url":v1,"request_url":format!("{v1}/chat/completions"),"api_key_env":"CODEX_MIXIN_REASONIX_API_KEY","models":models.iter().map(|m|&m.id).collect::<Vec<_>>(),"default":models[0].id,"model_overrides":overrides}));
    let reference = format!("{PROVIDER}/{}", models[0].id);
    let mut fields = vec![
        field(&["providers"], json!(providers)),
        field(&["default_model"], json!(reference)),
        field(&["agent", "planner_model"], json!(reference)),
    ];
    if let Some(access) = config.get("desktop").and_then(|d| d.get("provider_access")) {
        let mut access = access
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Reasonix desktop.provider_access must be an array"))?;
        if !access.iter().any(|v| v == PROVIDER) {
            access.push(json!(PROVIDER));
        }
        fields.push(field(&["desktop", "provider_access"], json!(access)));
    }
    Ok(vec![
        patch(path, Format::Toml, fields),
        patch(
            dir.join(".env"),
            Format::Env,
            vec![field(
                &["CODEX_MIXIN_REASONIX_API_KEY"],
                json!(serde_json::to_string(key)?),
            )],
        ),
    ])
}

fn minimax(dir: &Path, v1: &str, key: &str, models: &[FrameworkModel]) -> Vec<Patch> {
    let catalog: Map<String,Value> = models.iter().map(|m|(crate::provider::flat_model_id(&m.id),json!({"name":m.name,"enabled":true,"limit":{"context":m.context_window,"output":m.max_tokens},"capabilities":{"support_image":m.supports_image}}))).collect();
    vec![patch(
        dir.join("config.yaml"),
        Format::Yaml,
        vec![
            field(
                &["custom_provider", PROVIDER],
                json!({"name":PROVIDER,"kind":"custom","enabled":true,"api":"openai-completions","options":{"baseURL":v1,"apiKey":key,"authMode":"api-key"},"models":catalog}),
            ),
            field(
                &["defaultModel"],
                json!(format!(
                    "custom_provider:{PROVIDER}/{}",
                    crate::provider::flat_model_id(&models[0].id)
                )),
            ),
        ],
    )]
}

fn keyless(id: &str, v1: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let dir = match id {
        "commandcode" => home_path("", ".commandcode")?,
        "fx" => home_path("", ".fx")?,
        "empryo" => home_path("", ".empryo")?,
        "ante" => home_path("ANTE_HOME", ".ante")?,
        _ => anyhow::bail!("unknown keyless framework {id}"),
    };
    keyless_at(id, &dir, v1, models)
}

fn keyless_at(
    id: &str,
    dir: &Path,
    v1: &str,
    models: &[FrameworkModel],
) -> anyhow::Result<Vec<Patch>> {
    let first = &models[0];
    Ok(match id {
        "commandcode" => {
            let auth = read_config(&dir.join("auth.json"), Format::Json)?;
            anyhow::ensure!(
                auth.get(PROVIDER).is_none() && auth.pointer("/provider/codex-mixin").is_none(),
                "Command Code already has a stored codex-mixin credential; remove that native credential before connecting"
            );
            vec![
                patch(
                    dir.join("providers.json"),
                    Format::Json,
                    vec![field(
                        &["provider", PROVIDER],
                        json!({"name":PROVIDER,"api":"openai-completions","baseURL":v1,"apiKey":if cfg!(feature="client-auth") {json!("$CODEX_MIXIN_COMMANDCODE_API_KEY")} else {json!(false)},"models":Value::Object(models.iter().map(|m|(m.id.clone(),json!({"name":m.name}))).collect())}),
                    )],
                ),
                patch(
                    dir.join("settings.json"),
                    Format::Json,
                    vec![
                        field(&["modelProvider"], json!(PROVIDER)),
                        field(&["model"], json!(format!("{PROVIDER}/{}", first.id))),
                    ],
                ),
            ]
        }
        "fx" => {
            const MAX_MODELS: usize = 256;
            anyhow::ensure!(
                models.len() <= MAX_MODELS,
                "fx accepts at most {MAX_MODELS} selected models"
            );
            let metadata: serde_json::Map<String, Value> = models.iter().map(|m| {
                let mut entry = json!({"supports_tool_use":true,"supports_vision":m.supports_image,"context_window":m.context_window});
                if m.max_tokens < m.context_window {
                    entry["max_output_tokens"] = json!(m.max_tokens);
                }
                (m.id.clone(), entry)
            }).collect();
            vec![patch(
                dir.join("settings.json"),
                Format::Json,
                vec![
                    field(&["provider"], json!(PROVIDER)),
                    field(&["models", PROVIDER], json!(first.id)),
                    field(
                        &["providers", PROVIDER],
                        json!({"protocol":"openai-chat-completions","base_url":v1,"auth":if cfg!(feature="client-auth") {json!({"type":"bearer","env":"CODEX_MIXIN_FX_API_KEY"})} else {json!({"type":"none"})},"tool_choice_mode":"send","model_metadata":metadata}),
                    ),
                ],
            )]
        }
        "empryo" => {
            let path = dir.join("config.json");
            let doc = read_config(&path, Format::Json)?;
            let mut providers = match doc.get("providers") {
                None => Vec::new(),
                Some(value) => value
                    .as_array()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("Empryo providers must be an array"))?,
            };
            anyhow::ensure!(
                !providers.iter().any(|p| p["id"] == PROVIDER),
                "unmanaged Empryo provider collision"
            );
            let mut provider = json!({"id":PROVIDER,"name":PROVIDER,"baseURL":v1,"modelsAPI":format!("{v1}/models")});
            if cfg!(feature = "client-auth") {
                provider["envVar"] = json!("CODEX_MIXIN_EMPRYO_API_KEY");
            }
            providers.push(provider);
            vec![patch(
                path,
                Format::Json,
                vec![
                    field(&["providers"], json!(providers)),
                    field(&["defaultModel"], json!(format!("{PROVIDER}/{}", first.id))),
                ],
            )]
        }
        "ante" => {
            vec![
                patch(
                    dir.join("catalog.json"),
                    Format::Json,
                    vec![field(
                        &["providers", PROVIDER],
                        json!({"display_name":PROVIDER,"base_url":v1,"wire_style":"OpenAiCompatible","auth":if cfg!(feature="client-auth") {json!({"bearer":{"env_key":"CODEX_MIXIN_ANTE_API_KEY"}})} else {Value::Null},"preferred_models":models.iter().map(|m|json!({"id":m.id,"display_name":m.name,"context_limit":m.context_window,"max_tokens":m.max_tokens,"support_vision":m.supports_image})).collect::<Vec<_>>()}),
                    )],
                ),
                patch(
                    dir.join("settings.json"),
                    Format::Json,
                    vec![
                        field(&["provider"], json!(PROVIDER)),
                        field(&["model"], json!(first.id)),
                    ],
                ),
            ]
        }
        _ => anyhow::bail!("unknown keyless framework {id}"),
    })
}

fn cline(v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let dir = home_path("CLINE_DIR", ".cline")?;
    let data = match std::env::var_os("CLINE_DATA_DIR") {
        Some(_) => home_path("CLINE_DATA_DIR", "")?,
        None => dir.join("data"),
    };
    cline_at(&data, v1, key, models)
}

fn cline_at(
    data: &Path,
    v1: &str,
    key: &str,
    models: &[FrameworkModel],
) -> anyhow::Result<Vec<Patch>> {
    const SLOT: &str = "openai-compatible";
    let settings = data.join("settings/providers.json");
    let doc = read_config(&settings, Format::Json)?;
    anyhow::ensure!(
        doc.get("version").is_none_or(|v| v == 1),
        "unsupported Cline provider settings version"
    );
    let first = &models[0];
    let entries: serde_json::Map<String, Value> = models.iter().map(|m| {
        let mut capabilities = vec!["streaming", "tools"];
        if m.supports_image { capabilities.push("images"); }
        (m.id.clone(), json!({"id":m.id,"name":m.name,"contextWindow":m.context_window,"maxTokens":m.max_tokens,"capabilities":capabilities}))
    }).collect();
    let mut fields = vec![
        field(&["lastUsedProvider"], json!(SLOT)),
        field(
            &["providers", SLOT],
            json!({"settings":{"provider":SLOT,"apiKey":key,"model":first.id,"baseUrl":v1,"headers":{"User-Agent":"cline"}},"updatedAt":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"tokenSource":"manual"}),
        ),
    ];
    if doc.get("version").is_none() {
        fields.push(field(&["version"], json!(1)));
    }
    let mut out = vec![
        patch(settings, Format::Json, fields),
        patch(
            data.join("settings/models.json"),
            Format::Json,
            vec![field(
                &["providers", SLOT],
                json!({"provider":{"name":PROVIDER,"baseUrl":v1,"defaultModelId":first.id},"models":entries}),
            )],
        ),
    ];
    let state = data.join("globalState.json");
    if state.exists() {
        // Extension state wins over the shared provider file. Update both modes,
        // including cached model metadata, rather than claiming CLI-only success.
        let info = json!({"maxTokens":first.max_tokens,"contextWindow":first.context_window,"supportsImages":first.supports_image,"supportsPromptCache":false});
        out.push(patch(
            state,
            Format::Json,
            vec![
                field(&["actModeApiProvider"], json!("openai")),
                field(&["planModeApiProvider"], json!("openai")),
                field(&["actModeOpenAiModelId"], json!(first.id)),
                field(&["planModeOpenAiModelId"], json!(first.id)),
                field(&["actModeOpenAiModelInfo"], info.clone()),
                field(&["planModeOpenAiModelInfo"], info),
                field(&["openAiBaseUrl"], json!(v1)),
                field(&["openAiHeaders"], json!({"User-Agent":"cline"})),
            ],
        ));
        out.push(patch(
            data.join("secrets.json"),
            Format::Json,
            vec![field(&["openAiApiKey"], json!(key))],
        ));
    }
    Ok(out)
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
    let (path, data) = crate::platform::crush_config_paths()?;
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
    let dir = crate::platform::copilot_config_root()?;
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

fn vscodium_vendor(root: &Path) -> anyhow::Result<&'static str> {
    let index = read_config(&root.join("extensions.json"), Format::Json)?;
    let entries = index.as_array().ok_or_else(|| {
        anyhow::anyhow!("VSCodium requires an installed Copilot Chat extension index")
    })?;
    let installed: Vec<_> = entries
        .iter()
        .filter(|e| {
            e.pointer("/identifier/id")
                .and_then(Value::as_str)
                .is_some_and(|s| s.eq_ignore_ascii_case("github.copilot-chat"))
        })
        .collect();
    anyhow::ensure!(
        installed.len() == 1,
        "VSCodium requires exactly one active Copilot Chat extension"
    );
    let relative = installed[0]["relativeLocation"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("VSCodium extension index must contain relativeLocation"))?;
    let mut components = Path::new(relative).components();
    anyhow::ensure!(
        matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none()
            && !relative.contains(['/', '\\']),
        "unsafe VSCodium extension location"
    );
    let package = read_config(&root.join(relative).join("package.json"), Format::Json)?;
    anyhow::ensure!(
        package["publisher"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("github"))
            && package["name"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("copilot-chat")),
        "VSCodium extension identity mismatch"
    );
    let providers = package
        .pointer("/contributes/languageModelChatProviders")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            anyhow::anyhow!("installed Copilot Chat does not declare custom model providers")
        })?;
    for vendor in ["customendpoint", "customoai"] {
        if providers.iter().any(|p| p["vendor"] == vendor) {
            return Ok(vendor);
        }
    }
    anyhow::bail!("installed Copilot Chat supports neither customendpoint nor customoai")
}

fn vscode_group(vendor: &str, v1: &str, key: &str, models: &[FrameworkModel]) -> Value {
    let list: Vec<_> = models.iter().map(|m| {
        let output = m.max_tokens.min(m.context_window / 4);
        let mut entry = json!({"id":m.id,"name":m.name,"url":format!("{v1}/chat/completions"),"toolCalling":true,"vision":m.supports_image,"maxOutputTokens":output});
        if vendor == "customoai" {
            entry["maxInputTokens"] = json!(m.context_window - output);
            entry["requestHeaders"] = json!({"x-api-key":key});
        } else {
            entry["contextWindow"] = json!(m.context_window);
            entry["requestHeaders"] = json!({"Authorization":format!("Bearer {key}")});
        }
        entry
    }).collect();
    let mut group = json!({"name":PROVIDER,"vendor":vendor,"models":list});
    if vendor == "customendpoint" {
        group["apiType"] = json!("chat-completions");
    }
    group
}

fn vscode(id: &str, v1: &str, key: &str, models: &[FrameworkModel]) -> anyhow::Result<Vec<Patch>> {
    let vendor = if id == "vscodium" {
        vscodium_vendor(&crate::platform::vscodium_extensions_root()?)?
    } else {
        "customendpoint"
    };
    let folder = match id {
        "vscode-insiders" => "Code - Insiders",
        "vscodium" => "VSCodium",
        _ => "Code",
    };
    let root = crate::platform::framework_app_root()?;
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
    groups.push(vscode_group(vendor, v1, key, models));
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
    let root = crate::platform::framework_app_root()?;
    let dir = root.join("JetBrains/Air");
    let config = dir.join("codex-mixin-opencode.json");
    let binary = crate::platform::agent_cli::find_command("opencode")
        .or_else(|| {
            let binary = crate::platform::home_dir()
                .join(".opencode/bin")
                .join(crate::platform::executable_file_name("opencode"));
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
    fn gemini_configuration_preserves_oauth_and_restores_original_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().canonicalize().unwrap();
        let settings = dir.join("settings.json");
        let environment = dir.join(".env");
        let before = "{\"security\":{\"auth\":{\"selectedType\":\"oauth-personal\"}},\"modelConfigs\":{\"modelDefinitions\":{\"native\":{\"displayName\":\"Keep\"}}},\"experimental\":{\"dynamicModelConfiguration\":false},\"unrelated\":true}\n";
        std::fs::write(&settings, before).unwrap();
        std::fs::write(&environment, "# preserve\nOTHER=value\n").unwrap();
        let journal = dir.join("journal.json");
        let mut models = model_fixture();
        let mut second = model_fixture().remove(0);
        second.id = "provider/second-model".into();
        second.max_tokens = 2_048;
        models.push(second);
        super::super::engine::apply(
            &journal,
            gemini_at(&dir, "http://127.0.0.1:8787", "fixture-key", &models),
        )
        .unwrap();
        let value = read_config(&settings, Format::Json).unwrap();
        assert_eq!(
            value.pointer("/security/auth/selectedType").unwrap(),
            "gemini-api-key"
        );
        assert_eq!(
            value.pointer("/model/name").unwrap(),
            "provider/model.with.dots"
        );
        assert_eq!(value["experimental"]["dynamicModelConfiguration"], true);
        for model in &models {
            let definition = &value["modelConfigs"]["modelDefinitions"][&model.id];
            assert_eq!(definition["isVisible"], true);
            assert_eq!(definition["tier"], "custom");
            let alias = &value["modelConfigs"]["customAliases"][&model.id]["modelConfig"];
            assert_eq!(alias["model"], model.id);
            assert_eq!(
                alias["generateContentConfig"]["maxOutputTokens"],
                model.max_tokens
            );
        }
        assert_eq!(
            value["modelConfigs"]["modelDefinitions"]["native"]["displayName"],
            "Keep"
        );
        let values = read_config(&environment, Format::Env).unwrap();
        assert_eq!(values["GEMINI_API_KEY"], "fixture-key");
        assert_eq!(values["GOOGLE_GEMINI_BASE_URL"], "http://127.0.0.1:8787");
        assert_eq!(values["OTHER"], "value");
        super::super::engine::remove(&journal).unwrap();
        assert_eq!(std::fs::read_to_string(settings).unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(environment).unwrap(),
            "# preserve\nOTHER=value\n"
        );
    }

    #[test]
    fn agy_custom_models_and_muse_transport_keep_literal_model_ids() {
        for id in ["agy", "muse"] {
            let temp = tempfile::tempdir().unwrap();
            let dir = temp.path().canonicalize().unwrap();
            let settings = dir.join("settings.json");
            let before = "{\"schema_version\":1,\"model\":\"vendor-native\",\"unrelated\":true}";
            std::fs::write(&settings, before).unwrap();
            let patches = if id == "agy" {
                agy_at(&dir, &model_fixture())
            } else {
                muse_at(&dir, "http://127.0.0.1:8787/v1", &model_fixture()[0]).unwrap()
            };
            let journal = dir.join("journal.json");
            super::super::engine::apply(&journal, patches).unwrap();
            let value = read_config(&settings, Format::Json).unwrap();
            if id == "agy" {
                assert_eq!(value["modelProvider"], "gemini");
                assert_eq!(
                    value["customModelsConfig"]["customModels"]["codex-mixin/provider/model.with.dots"]
                        ["modelName"],
                    "provider/model.with.dots"
                );
            } else {
                assert_eq!(
                    value["endpoint_transport"]["base_url"],
                    "http://127.0.0.1:8787/v1"
                );
                assert_eq!(
                    value["endpoint_transport"]["auth"],
                    if cfg!(feature = "client-auth") {
                        "bearer"
                    } else {
                        "none"
                    }
                );
            }
            super::super::engine::remove(&journal).unwrap();
            assert_eq!(std::fs::read_to_string(settings).unwrap(), before);
        }
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("settings.json"), "{\"schema_version\":2}").unwrap();
        assert!(muse_at(temp.path(), "http://127.0.0.1:8787/v1", &model_fixture()[0]).is_err());
    }
    fn model_fixture() -> Vec<FrameworkModel> {
        vec![FrameworkModel {
            id: "provider/model.with.dots".to_owned(),
            name: "Model".to_owned(),
            context_window: 128000,
            max_tokens: 8192,
            supports_image: true,
            protocol: crate::provider::ProviderProtocol::OpenAiChat,
            efforts: vec!["high".to_owned()],
        }]
    }

    #[test]
    fn cline_roundtrip_preserves_credentials_and_unrelated_edits() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("data");
        std::fs::create_dir_all(data.join("settings")).unwrap();
        let providers = data.join("settings/providers.json");
        let state = data.join("globalState.json");
        let secrets = data.join("secrets.json");
        let original = r#"{"version":1,"lastUsedProvider":"native","providers":{"native":{"settings":{"apiKey":"fixture-native"}},"openai-compatible":{"settings":{"apiKey":"fixture-original"}}},"other":true}"#;
        std::fs::write(&providers, original).unwrap();
        std::fs::write(&state, r#"{"actModeApiProvider":"native","unrelated":1}"#).unwrap();
        std::fs::write(
            &secrets,
            r#"{"openAiApiKey":"fixture-old","otherKey":"fixture-keep"}"#,
        )
        .unwrap();
        let manifest = root.join("journal.json");
        super::super::engine::apply(
            &manifest,
            cline_at(
                &data,
                "http://127.0.0.1:8787/v1",
                "fixture-managed",
                &model_fixture(),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(super::super::engine::is_managed(&manifest).unwrap());
        let value = read_config(&providers, Format::Json).unwrap();
        assert_eq!(
            value["providers"]["openai-compatible"]["settings"]["apiKey"],
            "fixture-managed"
        );
        assert_eq!(
            value["providers"]["native"]["settings"]["apiKey"],
            "fixture-native"
        );
        let mut value = read_config(&state, Format::Json).unwrap();
        value["unrelated"] = json!(2);
        std::fs::write(&state, serde_json::to_vec(&value).unwrap()).unwrap();
        super::super::engine::remove(&manifest).unwrap();
        super::super::engine::remove(&manifest).unwrap();
        assert_eq!(std::fs::read_to_string(providers).unwrap(), original);
        assert_eq!(
            read_config(&state, Format::Json).unwrap(),
            json!({"actModeApiProvider":"native","unrelated":2})
        );
        assert_eq!(
            read_config(&secrets, Format::Json).unwrap()["openAiApiKey"],
            "fixture-old"
        );
        assert!(!data.join("settings/models.json").exists());
    }

    #[test]
    fn cline_drift_or_unknown_version_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let manifest = root.join("journal.json");
        super::super::engine::apply(
            &manifest,
            cline_at(
                &root,
                "http://127.0.0.1:8787/v1",
                "fixture-key",
                &model_fixture(),
            )
            .unwrap(),
        )
        .unwrap();
        let path = root.join("settings/providers.json");
        let mut value = read_config(&path, Format::Json).unwrap();
        value["lastUsedProvider"] = json!("user-choice");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(super::super::engine::remove(&manifest).is_err());
        assert_eq!(
            read_config(&path, Format::Json).unwrap()["lastUsedProvider"],
            "user-choice"
        );
        value["version"] = json!(2);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            cline_at(
                &root,
                "http://127.0.0.1:8787/v1",
                "fixture-key",
                &model_fixture()
            )
            .is_err()
        );
    }

    #[test]
    fn reasonix_credentials_and_catalog_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let config = root.join("config.toml");
        let env = root.join(".env");
        let original = "default_model = 'native/model'\n[desktop]\nprovider_access = ['native']\n[[providers]]\nname = 'native'\nkind = 'openai'\nbase_url = 'https://example.test/v1'\nmodel = 'model'\n";
        let env_original = "# keep me\nexport NATIVE_KEY='own secret' # keep\n";
        std::fs::write(&config, original).unwrap();
        std::fs::write(&env, env_original).unwrap();
        let journal = root.join("journal.json");
        super::super::engine::apply(
            &journal,
            reasonix(
                &root,
                "http://127.0.0.1:8787/v1",
                "fixture-key",
                &model_fixture(),
            )
            .unwrap(),
        )
        .unwrap();
        let parsed = read_config(&config, Format::Toml).unwrap();
        assert_eq!(parsed["providers"][0]["name"], "native");
        assert_eq!(
            parsed["providers"][1]["api_key_env"],
            "CODEX_MIXIN_REASONIX_API_KEY"
        );
        assert_eq!(
            parsed["agent"]["planner_model"],
            "codex-mixin/provider/model.with.dots"
        );
        assert_eq!(
            parsed["desktop"]["provider_access"],
            json!(["native", "codex-mixin"])
        );
        assert!(
            !std::fs::read_to_string(&config)
                .unwrap()
                .contains("fixture-key")
        );
        assert!(
            std::fs::read_to_string(&env)
                .unwrap()
                .contains("export NATIVE_KEY='own secret' # keep")
        );
        assert!(
            reasonix(
                &root,
                "http://127.0.0.1:8787/v1",
                "fixture-key",
                &model_fixture()
            )
            .is_err()
        );
        super::super::remove("reasonix", &journal).unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&env).unwrap(), env_original);
    }

    #[test]
    fn minimax_flat_catalog_and_selection_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let config = root.join("config.yaml");
        let original =
            "model: native/model\ncustom_provider:\n  native:\n    models:\n      own.model: {}\n";
        std::fs::write(&config, original).unwrap();
        let journal = root.join("journal.json");
        super::super::engine::apply(
            &journal,
            minimax(
                &root,
                "http://127.0.0.1:8787/v1",
                "fixture-key",
                &model_fixture(),
            ),
        )
        .unwrap();
        let parsed = read_config(&config, Format::Yaml).unwrap();
        let flat = crate::provider::flat_model_id("provider/model.with.dots");
        assert!(!flat.contains('/'));
        assert_eq!(
            parsed["defaultModel"],
            format!("custom_provider:codex-mixin/{flat}")
        );
        assert_eq!(
            parsed["custom_provider"][PROVIDER]["models"][&flat]["limit"]["context"],
            128000
        );
        assert_eq!(
            parsed["custom_provider"][PROVIDER]["options"]["baseURL"],
            "http://127.0.0.1:8787/v1"
        );
        assert!(parsed["custom_provider"]["native"].is_object());
        super::super::remove("minimax-code", &journal).unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
    }

    #[test]
    fn environment_auth_adapters_roundtrip_without_literal_secrets() {
        for id in ["commandcode", "fx", "empryo", "ante"] {
            assert!(super::super::require_supported(id).is_ok());
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let manifest = root.join("journal.json");
            let mut patches =
                keyless_at(id, &root, "http://127.0.0.1:8787/v1", &model_fixture()).unwrap();
            let paths: Vec<_> = patches.iter().map(|p| p.path.clone()).collect();
            for path in &paths {
                std::fs::write(path, r#"{"unrelated":"keep"}"#).unwrap();
            }
            let mut expected_environment = std::collections::BTreeMap::new();
            if id == "commandcode" {
                expected_environment.insert("CMD_LOCAL_ONLY".to_owned(), "1".to_owned());
            }
            if cfg!(feature = "client-auth") {
                expected_environment.insert(
                    format!("CODEX_MIXIN_{}_API_KEY", id.to_uppercase()),
                    "fixture-key".to_owned(),
                );
            }
            if !expected_environment.is_empty() {
                patches.push(Patch {
                    path: manifest.with_extension("launch.json"),
                    format: Format::Json,
                    fields: vec![(
                        vec![],
                        json!({
                            "environment":expected_environment,
                            "model":"provider/model.with.dots"
                        }),
                    )],
                });
            }
            super::super::engine::apply(&manifest, patches).unwrap();
            let serialized = paths
                .iter()
                .map(|path| std::fs::read_to_string(path).unwrap())
                .collect::<String>();
            if cfg!(feature = "client-auth") {
                assert!(serialized.contains(&format!("CODEX_MIXIN_{}_API_KEY", id.to_uppercase())));
            }
            assert!(super::super::engine::is_managed(&manifest).unwrap());
            if super::super::startup::required(id) {
                assert_eq!(
                    super::super::status(id, &manifest).unwrap().state,
                    "pending"
                );
                super::super::startup::install_at(
                    id,
                    &manifest,
                    vec![(root.join("profile"), format!("# owned startup {id}\n"))],
                )
                .unwrap();
            }
            assert_eq!(
                super::super::status(id, &manifest).unwrap().state,
                "managed"
            );
            let (environment, model) = super::super::launch_environment(id, &manifest).unwrap();
            assert_eq!(environment, expected_environment);
            assert_eq!(
                model.as_deref(),
                (!expected_environment.is_empty()).then_some("provider/model.with.dots")
            );
            super::super::remove(id, &manifest).unwrap();
            assert!(!manifest.with_extension("launch.json").exists());
            for path in paths {
                assert_eq!(
                    std::fs::read_to_string(path).unwrap(),
                    r#"{"unrelated":"keep"}"#
                );
            }
        }
    }

    #[test]
    fn vscodium_checks_installed_identity_and_provider_schema() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        assert!(vscodium_vendor(&root).is_err());
        let extension = root.join("github.copilot-chat-1.0");
        std::fs::create_dir(&extension).unwrap();
        std::fs::write(root.join("extensions.json"), r#"[{"identifier":{"id":"GitHub.copilot-chat"},"relativeLocation":"github.copilot-chat-1.0"}]"#).unwrap();
        for vendor in ["customendpoint", "customoai", "unknown"] {
            std::fs::write(extension.join("package.json"), serde_json::to_vec(&json!({"publisher":"GitHub","name":"copilot-chat","contributes":{"languageModelChatProviders":[{"vendor":vendor}]}})).unwrap()).unwrap();
            if vendor == "unknown" {
                assert!(vscodium_vendor(&root).is_err());
                continue;
            }
            assert_eq!(vscodium_vendor(&root).unwrap(), vendor);
            let group = vscode_group(
                vendor,
                "http://127.0.0.1:8787/v1",
                "fixture-key",
                &model_fixture(),
            );
            assert_eq!(group["models"][0]["id"], "provider/model.with.dots");
            if vendor == "customoai" {
                assert_eq!(
                    group["models"][0]["requestHeaders"]["x-api-key"],
                    "fixture-key"
                );
                assert_eq!(group["models"][0]["maxInputTokens"], 119808);
                assert!(group.get("apiType").is_none());
            }
        }
        std::fs::write(
            root.join("extensions.json"),
            r#"[{"identifier":{"id":"github.copilot-chat"},"relativeLocation":"../escape"}]"#,
        )
        .unwrap();
        assert!(vscodium_vendor(&root).is_err());
    }

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
