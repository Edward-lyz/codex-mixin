use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{anyhow, ensure};
use serde::{Deserialize, Serialize};

use super::{installed_sources, npm_name, valid_npm_name};
use crate::pi_plugins::{self, NPM_REGISTRY};

const NPM_INDEX: &str = "https://registry.npmjs.org";
const FEATURED: &[(&str, &str)] = &[
    ("pi-opencode-free", "OpenCode Zen free-tier model provider"),
    ("@czottmann/pi-zai-api", "Z.AI API model provider"),
    ("pi-cursor-sdk", "Cursor SDK agent provider"),
    ("@sinamtz/pi-minimax-provider", "MiniMax model provider"),
    (
        "pi-mcp-adapter",
        "Model Context Protocol adapter for Pi sessions",
    ),
];

#[derive(Serialize)]
pub struct MarketPackage {
    pub name: String,
    pub version: Option<String>,
    pub description: String,
    pub installed: bool,
}

#[derive(Deserialize)]
struct SearchResult {
    objects: Vec<SearchObject>,
}

#[derive(Deserialize)]
struct SearchObject {
    package: Package,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    keywords: Option<Vec<String>>,
}

pub async fn search(query: &str, limit: usize) -> anyhow::Result<Vec<MarketPackage>> {
    ensure!(query.len() <= 256, "Pi market query is too long");
    ensure!(
        (1..=100).contains(&limit),
        "Pi market limit must be 1 to 100"
    );
    let root = pi_plugins::root()?;
    let installed: HashSet<String> = installed_sources(&root)
        .await?
        .iter()
        .map(|source| npm_name(source).to_owned())
        .collect();
    let query = query.trim().to_ascii_lowercase();
    let mut rows: Vec<MarketPackage> = Vec::new();
    let npm_query = format!("keywords:pi-package {query}");
    let (mirror, upstream) = tokio::join!(
        fetch(
            NPM_REGISTRY,
            if query.is_empty() {
                "pi-package"
            } else {
                &query
            },
            limit
        ),
        fetch(NPM_INDEX, &npm_query, limit),
    );
    let fetch_error = match (&mirror, &upstream) {
        (Err(_), Err(error)) => Some(error.to_string()),
        _ => None,
    };
    for package in mirror.into_iter().chain(upstream).flatten() {
        let description = package.description.unwrap_or_default();
        if !valid_npm_name(&package.name)
            || !package
                .keywords
                .is_some_and(|keywords| keywords.iter().any(|keyword| keyword == "pi-package"))
            || !searchable(&package.name, &description, &query)
        {
            continue;
        }
        if let Some(row) = rows.iter_mut().find(|row| row.name == package.name) {
            if row.version.is_none() {
                row.version = Some(package.version);
            }
        } else {
            rows.push(MarketPackage {
                installed: installed.contains(&package.name),
                name: package.name,
                version: Some(package.version),
                description,
            });
        }
    }
    for (name, description) in FEATURED {
        if searchable(name, description, &query) && !rows.iter().any(|row| row.name == *name) {
            rows.push(MarketPackage {
                name: (*name).to_owned(),
                version: None,
                description: (*description).to_owned(),
                installed: installed.contains(*name),
            });
        }
    }
    if rows.is_empty()
        && let Some(error) = fetch_error
    {
        return Err(anyhow!("Pi package index unavailable: {error}"));
    }
    rows.truncate(limit);
    Ok(rows)
}

fn searchable(name: &str, description: &str, query: &str) -> bool {
    let label = format!("{name} {description}").to_ascii_lowercase();
    query.split_whitespace().all(|term| label.contains(term))
}

async fn fetch(registry: &str, query: &str, limit: usize) -> anyhow::Result<Vec<Package>> {
    static HTTP: LazyLock<Result<reqwest::Client, reqwest::Error>> = LazyLock::new(|| {
        crate::platform::proxy::auto_proxy(reqwest::Client::builder())
            .timeout(Duration::from_secs(4))
            .build()
    });
    let client = HTTP
        .as_ref()
        .map_err(|error| anyhow!("create package index client: {error}"))?;
    let size = (limit * 2).min(200).to_string();
    let response: SearchResult = client
        .get(format!("{registry}/-/v1/search"))
        .query(&[("text", query), ("size", size.as_str())])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(response
        .objects
        .into_iter()
        .map(|object| object.package)
        .collect())
}
