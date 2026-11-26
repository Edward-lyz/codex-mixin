//! The managed DUCX auth carrier is a subprocess that can stall when the
//! machine is starved. These tests run the real gateway (or the real model
//! discovery) against a scripted `ducx` launcher that speaks the carrier's
//! pipe protocol, and check how often and for how long it is started.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use axum::{Json, Router, routing::post};
use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
use codex_mixin::provider::{
    BaiduAuthBridge, ProviderDefinition, ProviderPreset, discover_provider_models,
};
use serde_json::json;

mod common;

const CARRIER_LINE: &str = r#"CODEX_MIXIN_DUCX_AUTH_V1={"version":1,"model_token":"fixture-token","custom_header":"fixture-header"}"#;

/// Installs a scripted `ducx` in the managed layout. Every run appends its PID
/// to the returned log, then runs `body`.
fn install_ducx(root: &Path, body: &str) -> (PathBuf, PathBuf) {
    let executable = root.join("home/.codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    let log = root.join("ducx-runs.log");
    fs::write(
        &executable,
        format!("#!/bin/sh\necho \"$$\" >> '{}'\n{body}\n", log.display()),
    )
    .unwrap();
    codex_mixin::platform::make_executable(&executable).unwrap();
    (executable, log)
}

fn runs(log: &Path) -> usize {
    fs::read_to_string(log).map_or(0, |text| text.lines().count())
}

fn ducx_provider(executable: &Path, base_url: &str) -> ProviderDefinition {
    let mut provider = ProviderPreset::BaiduOneApi.create("baidu-oneapi", "fixture-key");
    provider.base_url = base_url.to_owned();
    provider.quota_url = None;
    provider.request_policy.baidu_auth_bridge = Some(BaiduAuthBridge::DucxLoopback);
    provider.request_policy.ducx_executable = Some(executable.to_owned());
    provider
}

/// Ends the carriers a failing test leaves running.
struct Leftovers(PathBuf);

impl Drop for Leftovers {
    fn drop(&mut self) {
        for pid in fs::read_to_string(&self.0).unwrap_or_default().lines() {
            let command = Command::new("ps")
                .args(["-o", "command=", "-p", pid])
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
                .unwrap_or_default();
            if command.contains("sleep") {
                let _ = Command::new("kill").args(["-9", pid]).status();
            }
        }
    }
}

async fn start_gateway(root: &Path, provider: ProviderDefinition) -> tokio::process::Child {
    save_stored_config_to_path(
        &root.join("config.json"),
        &StoredGatewayConfig {
            providers: vec![provider],
            ..Default::default()
        },
    )
    .unwrap();
    let child = common::tokio_cli(root)
        .arg("start")
        .arg("--log-file")
        .arg(root.join("gateway.log"))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while !root.join("runtime.json").is_file() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("gateway did not publish runtime metadata");
    child
}

#[tokio::test]
async fn model_discovery_reuses_headers_minted_within_the_last_minute() {
    async fn available_models() -> Json<serde_json::Value> {
        Json(json!({"success": true, "data": [{
            "model": "fixture-model",
            "capability": {
                "supports_image": false,
                "context_window": 128000,
                "ratio": "1",
                "model_description": "fixture"
            },
            "capability_set": [],
            "price_type": "fixture"
        }]}))
    }
    let root = tempfile::tempdir().unwrap();
    let (executable, log) = install_ducx(root.path(), &format!("printf '%s\\n' '{CARRIER_LINE}'"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let app = Router::new().route("/openapi/v2/available_models", post(available_models));
        axum::serve(listener, app).await.unwrap();
    });
    let provider = ducx_provider(&executable, &format!("http://{address}"));
    let client = reqwest::Client::new();

    for _ in 0..2 {
        let models = discover_provider_models(&client, &provider).await.unwrap();
        assert_eq!(models[0].id, "fixture-model");
    }

    assert_eq!(
        runs(&log),
        1,
        "the second discovery must reuse the headers of the first"
    );
    server.abort();
}

#[tokio::test]
async fn gateway_gives_up_on_a_stuck_carrier_after_thirty_seconds() {
    let root = tempfile::tempdir().unwrap();
    let (executable, log) = install_ducx(root.path(), "exec sleep 600");
    let _leftovers = Leftovers(log);
    let _gateway = start_gateway(
        root.path(),
        ducx_provider(&executable, "http://127.0.0.1:9"),
    )
    .await;
    let gateway_log = root.path().join("gateway.log");

    let started = Instant::now();
    while !fs::read_to_string(&gateway_log)
        .is_ok_and(|text| text.contains("managed provider auth warmup failed"))
    {
        assert!(
            started.elapsed() < Duration::from_secs(50),
            "the startup warmup is still waiting for a stuck DUCX carrier"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test]
async fn failing_carrier_is_not_retried_on_the_next_refresh_tick() {
    let root = tempfile::tempdir().unwrap();
    let (executable, log) = install_ducx(root.path(), "exit 1");
    let _gateway = start_gateway(
        root.path(),
        ducx_provider(&executable, "http://127.0.0.1:9"),
    )
    .await;

    let started = Instant::now();
    while runs(&log) == 0 {
        assert!(started.elapsed() < Duration::from_secs(20));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut settled = runs(&log);
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        if runs(&log) == settled {
            break;
        }
        settled = runs(&log);
    }
    // Provider models refresh every 30 s. A provider whose refresh just
    // failed has to wait longer than the next tick.
    tokio::time::sleep(Duration::from_secs(40)).await;

    assert_eq!(
        runs(&log),
        settled,
        "a failed carrier run was retried on the next 30 s refresh tick"
    );
}
