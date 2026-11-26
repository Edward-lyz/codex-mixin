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

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JudgeSynthesis {
    pub(super) points: Vec<JudgeSynthesisPoint>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JudgeSynthesisPoint {
    pub(super) title: String,
    pub(super) body: String,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FusionJudgeStatus {
    Completed,
    Failed,
    TimedOut,
    Skipped,
}

impl FusionJudgeStatus {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::TimedOut => "Timed Out",
            Self::Skipped => "Skipped",
        }
    }
}

#[derive(Debug)]
pub(super) struct FusionJudgeDetail {
    pub(super) model: String,
    pub(super) status: FusionJudgeStatus,
    pub(super) text: String,
}

#[derive(Debug)]
pub(super) struct RenderedFusionDetail {
    pub(super) item: Value,
    pub(super) events: Vec<Bytes>,
}
