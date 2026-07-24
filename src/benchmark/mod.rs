use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::{Instant, timeout_at};
use uuid::Uuid;

use crate::config::stored_config_path;
use crate::provider::{ProviderProtocol, ProviderQuotaParser, ProviderRuntime};
use crate::sse::SseDecoder;

mod manager;
mod runner;
mod types;

#[cfg(test)]
pub(crate) use types::BENCHMARK_PROMPT;
pub(crate) use types::BenchmarkTarget;
pub use types::{
    BENCHMARK_TARGET_OUTPUT_TOKENS, BenchmarkResultStatus, BenchmarkRunStatus,
    BenchmarkSnapshotResponse, ModelBenchmarkManager, ModelBenchmarkResult, ModelBenchmarkSnapshot,
    ProviderBenchmarkCost, StartBenchmarkRequest,
};

#[cfg(test)]
mod tests;
