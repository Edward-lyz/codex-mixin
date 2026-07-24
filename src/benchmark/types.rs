use super::*;

pub const BENCHMARK_TARGET_OUTPUT_TOKENS: u64 = 100;
pub(super) const BENCHMARK_FILE_VERSION: u64 = 2;
pub(crate) const BENCHMARK_PROMPT: &str = "Generate an endless stream of unrelated lowercase English words separated by single spaces. Do not count, explain, punctuate, repeat a fixed pattern, or conclude. Continue until the server cuts off generation.";

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkRunStatus {
    Running,
    Completed,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkResultStatus {
    Completed,
    TimedOut,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelBenchmarkResult {
    pub model: String,
    pub provider_id: String,
    pub provider_name: String,
    pub upstream_model: String,
    pub status: BenchmarkResultStatus,
    pub ttft_ms: Option<u64>,
    pub generation_ms: Option<u64>,
    pub total_ms: u64,
    pub output_tokens: Option<u64>,
    pub tps: Option<f64>,
    pub error: Option<String>,
    pub completed_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderBenchmarkCost {
    pub provider_id: String,
    pub currency: Option<String>,
    pub estimated_cost: Option<f64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelBenchmarkSnapshot {
    pub version: u64,
    pub run_id: String,
    pub status: BenchmarkRunStatus,
    pub started_at: u64,
    pub updated_at: u64,
    pub finished_at: Option<u64>,
    pub timeout_seconds: u64,
    pub target_output_tokens: u64,
    pub total_models: usize,
    pub current_model: Option<String>,
    pub results: Vec<ModelBenchmarkResult>,
    pub error: Option<String>,
    #[serde(default)]
    pub estimated_cost: Option<f64>,
    #[serde(default)]
    pub cost_currency: Option<String>,
    #[serde(default)]
    pub cost_error: Option<String>,
    #[serde(default)]
    pub provider_costs: Vec<ProviderBenchmarkCost>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StartBenchmarkRequest {
    pub timeout_seconds: u64,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BenchmarkSnapshotResponse {
    pub snapshot: Option<ModelBenchmarkSnapshot>,
}

#[derive(Clone)]
pub struct ModelBenchmarkManager {
    pub(super) snapshot_path: Arc<PathBuf>,
    pub(super) running: Arc<AtomicBool>,
    pub(super) snapshot_cache: Arc<RwLock<Option<ModelBenchmarkSnapshot>>>,
}

pub(super) struct RunningReset(pub(super) Arc<AtomicBool>);

impl Drop for RunningReset {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(super) struct BenchmarkMetrics {
    pub(super) ttft_ms: u64,
    pub(super) generation_ms: Option<u64>,
    pub(super) total_ms: u64,
    pub(super) output_tokens: u64,
    pub(super) tps: Option<f64>,
}

pub(super) struct BenchmarkAttemptFailure {
    pub(super) timed_out: bool,
    pub(super) message: String,
    pub(super) ttft_ms: Option<u64>,
    pub(super) total_ms: u64,
}

#[derive(Clone)]
pub(crate) struct BenchmarkTarget {
    pub(crate) catalog_slug: String,
    pub(crate) provider_id: String,
    pub(crate) provider_name: String,
    pub(crate) upstream_model_id: String,
    pub(crate) provider: ProviderRuntime,
}
