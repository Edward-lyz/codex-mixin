use super::*;

#[derive(Debug)]
pub(super) struct PanelResult {
    pub(super) index: usize,
    pub(super) model: String,
    pub(super) text: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PanelAnalysis {
    pub(super) findings: Vec<String>,
    pub(super) risks: Vec<String>,
    pub(super) recommendations: Vec<String>,
    pub(super) evidence: Vec<String>,
}

#[derive(Debug)]
pub(super) struct FusionDetail {
    pub(super) title: String,
    pub(super) text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FusionPanelStatus {
    Completed,
    Failed,
}

#[derive(Debug)]
pub(super) struct FusionPanelDetail {
    pub(super) index: usize,
    pub(super) model: String,
    pub(super) status: FusionPanelStatus,
    pub(super) text: String,
}

#[derive(Debug)]
pub(super) struct RenderedFusionDetail {
    pub(super) item: Value,
    pub(super) events: Vec<Bytes>,
}
