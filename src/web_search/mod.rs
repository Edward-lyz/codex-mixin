use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use futures_util::{StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::timeout;
use uuid::Uuid;

use crate::anthropic::ModelInfo;
use crate::config::{
    GatewayConfig, ProviderPreset, UpstreamAuthHeader, UpstreamKind, stored_config_path,
};
use crate::fusion::canonical_upstream_model_alias;
use crate::sse::SseDecoder;

mod capabilities;
mod probe;
mod storage;
mod types;

pub use types::{ModelWebSearchCapability, WebSearchCapabilities, WebSearchProbeSummary};

#[cfg(test)]
mod tests;
