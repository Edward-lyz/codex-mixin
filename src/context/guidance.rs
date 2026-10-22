use serde::{Deserialize, Serialize};

// Rules and nudge policy ported from ranxianglei/billion-context (MIT),
// 94f75ccea1b2e509a29ca46e44768dcf9364940f. License: prompts/LICENSE.
pub const GUIDANCE: &str = concat!(
    "<context_management>\n",
    "You have four tools that manage conversation context. Original messages remain archived and can be restored.\n",
    "- mixin_context_compress: replace older context with a summary; follow the tier rules in <context_state>. The rules call this tool `compress`.\n",
    "- mixin_context_decompress: restore a summary block or the original messages behind it.\n",
    "- mixin_context_search: search archived summaries and originals.\n",
    "- mixin_context_status: inspect token pressure, protected ranges, tier counts and compressible ranges.\n",
    "Never compress protected content or content the current step is actively using.\n",
    include_str!("prompts/philosophy.txt"),
    include_str!("prompts/tier1.txt"),
    "</context_management>\n",
);

/// This ablation has no context tools; do not advertise unavailable calls.
pub const GUIDANCE_WITHOUT_TOOLS: &str = "<context_management>\n\
The conversation transcript grows long. Keep every turn dense: preserve goals, constraints, decisions and their reasons, file paths, function names, commands, error messages and open TODOs, and drop repetition.\n\
</context_management>\n";

/// Marks a note that asks the model to compress, so the gateway can tell
/// which rounds are likely to end in an internal tool call.
pub const NUDGE_MARKER: &str = "compress consumed context";

const MIN_CONTEXT_RATIO: f64 = 0.45;
const MIN_GROWTH_TOKENS: u64 = 20_000;
const MIN_GROWTH_RATIO: f64 = 0.45;
const MIN_PRESSURE_TOKENS: u64 = 5_000;
const MIN_PRESSURE_RATIO: f64 = 0.01;
const TIER_GROWTH_MULTIPLIER: f64 = 1.5;
const TIER_COUNT_TRIGGERS: [u64; 3] = [u64::MAX, 1_000, 2_000];
pub(super) const MIN_RANGE_CHARS: usize = 5_000;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct NudgeState {
    baseline: u64,
    shown: u64,
    by_tier: [u64; 3],
}

#[derive(Clone, Debug, Default)]
pub(super) struct PendingTier {
    pub tokens: u64,
    pub blocks: u64,
    pub oldest: Option<(String, String)>,
}

impl NudgeState {
    /// Decide before stamping, as in the upstream nudge pipeline. Low-water
    /// references survive restarts; a successful compression clears them.
    pub(super) fn note(
        &mut self,
        tokens: u64,
        window: u64,
        settings: &crate::config::ContextSettings,
        tiers: &[PendingTier; 3],
        advise: bool,
    ) -> String {
        let usage = if window == 0 {
            0.0
        } else {
            tokens as f64 / window as f64
        };
        let mut note = if window == 0 {
            format!("current view: {tokens} estimated tokens; window unknown.\n")
        } else {
            format!(
                "current view: {tokens} estimated tokens of {window} ({:.0}%).\n",
                usage * 100.0
            )
        };
        if !advise {
            return note;
        }
        let tier = self.select_tier(tokens, window, settings, tiers);
        // Every reference follows the most recent valley, never rising unless
        // a nudge is actually shown. This prevents stale-high growth debt.
        if self.baseline == 0 || tokens < self.baseline {
            self.baseline = tokens;
        }
        if self.shown > 0 {
            self.shown = self.shown.min(tokens);
        }
        for stamp in &mut self.by_tier {
            *stamp = (*stamp).min(tokens);
        }
        let Some(tier) = tier else {
            note.push_str("No actionable compression nudge; keep working.\n");
            return note;
        };
        self.shown = tokens;
        self.by_tier[tier] = tokens;
        let urgency = if usage >= settings.urgent_ratio {
            "EMERGENCY"
        } else if usage >= settings.pressure_ratio {
            "OVER-LIMIT"
        } else {
            "GROWTH"
        };
        note.push_str(&format!(
            "{urgency} T{}: {} pending tokens; {NUDGE_MARKER} following these rules.\n",
            tier + 1,
            tiers[tier].tokens
        ));
        if let Some((start, end)) = &tiers[tier].oldest {
            note.push_str(&format!(
                "oldest unprotected range: {start}..{end} (t{}).\n",
                tier + 1
            ));
        }
        note.push_str(include_str!("prompts/tier1.txt"));
        match tier {
            1 => note.push_str(include_str!("prompts/tier2.txt")),
            2 => note.push_str(include_str!("prompts/tier3.txt")),
            _ => {}
        }
        note
    }

    fn select_tier(
        &self,
        tokens: u64,
        window: u64,
        settings: &crate::config::ContextSettings,
        tiers: &[PendingTier; 3],
    ) -> Option<usize> {
        let usage = if window == 0 {
            0.0
        } else {
            tokens as f64 / window as f64
        };
        let allowed = usize::from(settings.max_tier);
        let step = settings.growth_threshold_tokens;
        let minimum_growth = MIN_GROWTH_TOKENS.max((step as f64 * MIN_GROWTH_RATIO).ceil() as u64);
        let reference = if self.shown > 0 {
            self.shown
        } else if self.baseline > 0 {
            self.baseline
        } else {
            tokens
        };
        let growth = tokens.saturating_sub(reference);
        let max_pending = tiers.iter().map(|tier| tier.tokens).max().unwrap_or(0);
        if usage >= settings.pressure_ratio || usage >= settings.urgent_ratio {
            let minimum_benefit =
                MIN_PRESSURE_TOKENS.max((window as f64 * MIN_PRESSURE_RATIO).round() as u64);
            // Strict > preserves upstream T1-first tie breaking.
            let mut best = None;
            let mut pending = 0;
            for (index, tier) in tiers.iter().enumerate().take(allowed) {
                if tier.tokens > pending {
                    pending = tier.tokens;
                    best = Some(index);
                }
            }
            return best.filter(|_| pending >= minimum_benefit);
        }
        let first_sight = self.shown == 0
            && self.baseline == 0
            && usage >= MIN_CONTEXT_RATIO
            && max_pending >= step;
        if !first_sight && growth < minimum_growth {
            return None;
        }
        if tiers[0].tokens >= step {
            return Some(0);
        }
        let mass_threshold = (step as f64 * TIER_GROWTH_MULTIPLIER).round() as u64;
        for index in 1..allowed {
            let count_ready = tiers[index].blocks >= TIER_COUNT_TRIGGERS[index];
            let mass_ready = tiers[index].tokens >= mass_threshold
                && tiers[..index]
                    .iter()
                    .all(|lower| tiers[index].tokens > lower.tokens);
            // The upstream else-if arbitration does not fall through when a
            // higher-priority tier is ready but its own cadence is not met.
            if count_ready || mass_ready {
                let last = self.by_tier[index];
                return (last == 0 || tokens.saturating_sub(last) >= minimum_growth)
                    .then_some(index);
            }
        }
        None
    }
}

pub fn with_guidance(instructions: Option<&str>, guidance: &str) -> String {
    match instructions {
        Some(existing) if !existing.is_empty() => format!("{existing}\n\n{guidance}"),
        _ => guidance.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ContextSettings;

    fn pending(tokens: [u64; 3]) -> [PendingTier; 3] {
        tokens.map(|tokens| PendingTier {
            tokens,
            ..PendingTier::default()
        })
    }

    #[test]
    fn matches_upstream_nudge_decisions() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/context_nudge/expected.json"
        ))
        .expect("upstream fixture");
        for (index, case) in fixture["cases"]
            .as_array()
            .expect("cases")
            .iter()
            .enumerate()
        {
            let mut state: NudgeState =
                serde_json::from_value(case["before"].clone()).expect("state");
            let amounts: [u64; 3] =
                serde_json::from_value(case["pending"].clone()).expect("pending");
            let counts: [u64; 3] = serde_json::from_value(case["counts"].clone()).expect("counts");
            let mut tiers = pending(amounts);
            for (tier, blocks) in tiers.iter_mut().zip(counts) {
                tier.blocks = blocks;
            }
            let tokens = case["tokens"].as_u64().expect("tokens");
            let window = case["window"].as_u64().expect("window");
            let selected = state.select_tier(tokens, window, &ContextSettings::default(), &tiers);
            assert_eq!(
                serde_json::to_value(selected.map(|tier| tier + 1)).expect("tier"),
                case["tier"],
                "case {index}: {case}"
            );
            state.note(tokens, window, &ContextSettings::default(), &tiers, true);
            assert_eq!(
                serde_json::to_value(state).expect("stamps"),
                case["after"],
                "case {index}: {case}"
            );
        }
    }

    #[test]
    fn total_size_does_not_replace_growth() {
        let mut state = NudgeState::default();
        let settings = ContextSettings::default();
        let tiers = pending([60_000, 0, 0]);
        assert!(
            state
                .note(65_000, 500_000, &settings, &tiers, true)
                .contains("keep working")
        );
        assert!(
            state
                .note(87_499, 500_000, &settings, &tiers, true)
                .contains("keep working")
        );
        assert!(
            state
                .note(87_500, 500_000, &settings, &tiers, true)
                .contains("GROWTH T1")
        );
        assert!(
            state
                .note(87_500, 500_000, &settings, &tiers, true)
                .contains("keep working")
        );
        assert!(
            state
                .note(110_000, 500_000, &settings, &tiers, true)
                .contains("GROWTH T1")
        );
    }

    #[test]
    fn first_sight_requires_the_usage_band() {
        let settings = ContextSettings::default();
        let tiers = pending([50_000, 0, 0]);
        for (tokens, expected) in [(224_999, false), (225_000, true)] {
            let note = NudgeState::default().note(tokens, 500_000, &settings, &tiers, true);
            assert_eq!(note.contains("GROWTH T1"), expected, "{note}");
        }
    }

    #[test]
    fn pressure_requires_effective_benefit() {
        let settings = ContextSettings::default();
        for tokens in [375_000, 475_000] {
            for amount in [0, 4_999, 5_000] {
                let note = NudgeState::default().note(
                    tokens,
                    500_000,
                    &settings,
                    &pending([amount, 0, 0]),
                    true,
                );
                assert_eq!(
                    note.contains("pending tokens; compress"),
                    amount >= 5_000,
                    "{note}"
                );
            }
        }
        let note = NudgeState::default().note(
            950_000,
            1_000_000,
            &settings,
            &pending([9_999, 0, 0]),
            true,
        );
        assert!(note.contains("keep working"));
    }

    #[test]
    fn tiers_follow_mass_priority_and_pressure() {
        let settings = ContextSettings::default();
        for (amounts, expected) in [
            ([50_000, 90_000, 100_000], 0),
            ([49_999, 75_000, 100_000], 1),
            ([0, 74_999, 75_000], 2),
        ] {
            let note =
                NudgeState::default().note(250_000, 500_000, &settings, &pending(amounts), true);
            assert!(
                note.contains(&format!("GROWTH T{}", expected + 1)),
                "{note}"
            );
        }
        let note = NudgeState::default().note(
            475_000,
            500_000,
            &settings,
            &pending([60_000, 90_000, 100_000]),
            true,
        );
        assert!(note.contains("EMERGENCY T3"));
        assert!(note.contains("Open objectives:"));
        assert!(note.contains("TIER 3 COMPRESSION"));
    }

    #[test]
    fn baseline_follows_drops_and_unknown_windows() {
        let settings = ContextSettings::default();
        let tiers = pending([60_000, 0, 0]);
        let mut state = NudgeState::default();
        state.note(100_000, 0, &settings, &tiers, true);
        assert!(
            state
                .note(90_000, 0, &settings, &tiers, true)
                .contains("keep working")
        );
        assert!(
            state
                .note(112_499, 0, &settings, &tiers, true)
                .contains("keep working")
        );
        assert!(
            state
                .note(112_500, 0, &settings, &tiers, true)
                .contains("GROWTH T1")
        );
        assert!(
            state
                .note(90_000, 0, &settings, &tiers, true)
                .contains("keep working")
        );
        assert!(
            state
                .note(112_500, 0, &settings, &tiers, true)
                .contains("GROWTH T1")
        );
    }

    #[test]
    fn guidance_only_does_not_stamp_or_suggest_tools() {
        let mut state = NudgeState::default();
        let note = state.note(
            99_000,
            100_000,
            &ContextSettings::default(),
            &pending([90_000, 0, 0]),
            false,
        );
        assert!(!note.contains("compress consumed"));
        assert_eq!(state.baseline, 0);
        assert_eq!(with_guidance(None, GUIDANCE), GUIDANCE);
        assert!(
            with_guidance(Some("be brief"), GUIDANCE)
                .starts_with("be brief\n\n<context_management>")
        );
    }
}
