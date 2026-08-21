//! Prompt-prefix cache contract for provider-visible requests.
//!
//! Providers cache on the rendered prompt prefix: system prompt, tool
//! definitions, then the message sequence in order. A session keeps that cache
//! only while those regions stay byte-identical and every new turn is appended
//! at the tail. This module derives the shape from the exact bytes each
//! protocol serializes upstream and reports where a session lost its prefix, so
//! a cache miss has a concrete cause instead of a guess.

mod shape;
mod tracking;

pub(crate) use shape::CacheShape;
pub(crate) use tracking::{
    CacheShapeTracker, PrefixObservation, UpstreamCacheObserver, observe_upstream_cache_usage,
    record_provider_prefix,
};

#[cfg(test)]
use super::cache_usage::{TokenUsageAggregator, UpstreamCacheUsage};
#[cfg(test)]
use crate::{anthropic::MessageRequest, upstream::UpstreamRouting};
#[cfg(test)]
use bytes::Bytes;
#[cfg(test)]
use futures_util::StreamExt;
#[cfg(test)]
use serde_json::Value;
#[cfg(test)]
use shape::{ANTHROPIC_MESSAGES, PrefixReport, PrefixState, RegionDigest};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use tracking::{MIN_PROMPT_TOKENS_FOR_VERDICT, TRACKED_SESSIONS, TRACKED_SHAPES_PER_SESSION};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::{ContentBlock, Message};
    use serde_json::json;

    fn responses_request(instructions: &str, input: Value, tools: Value) -> Value {
        json!({
            "model": "provider-model",
            "instructions": instructions,
            "tools": tools,
            "input": input
        })
    }

    fn turn(text: &str) -> Value {
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":text}]})
    }

    fn tracker_with_first(request: &Value) -> (CacheShapeTracker, PrefixReport) {
        let tracker = CacheShapeTracker::default();
        let report = tracker.record("session", CacheShape::from_openai_responses(request));
        (tracker, report)
    }

    fn anthropic_request() -> MessageRequest {
        MessageRequest {
            model: "provider-model".to_owned(),
            max_tokens: 64,
            stream: true,
            speed: None,
            messages: vec![Message {
                role: "user".to_owned(),
                content: vec![ContentBlock::Text {
                    text: "say hi".to_owned(),
                }],
            }],
            system: Some(vec![ContentBlock::Text {
                text: "You are Codex.".to_owned(),
            }]),
            tools: vec![json!({"name":"exec_command"})],
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    #[test]
    fn region_digests_cover_the_exact_upstream_bytes() {
        let request = anthropic_request();
        let wire = String::from_utf8(serde_json::to_vec(&request).unwrap()).unwrap();

        // The digested regions must appear verbatim in the body `reqwest`
        // serializes, otherwise the hashes describe a shape the provider never
        // sees.
        let system = serde_json::to_string(request.system.as_ref().unwrap()).unwrap();
        let message = serde_json::to_string(&request.messages[0]).unwrap();
        assert!(wire.contains(&system), "system region missing from {wire}");
        assert!(
            wire.contains(&message),
            "message region missing from {wire}"
        );

        let shape = CacheShape::from_anthropic(&request);
        assert_eq!(shape.protocol, ANTHROPIC_MESSAGES);
        assert_eq!(shape.turns.len(), 1);
        // One digest per system block, so a newly appended developer block is
        // visible as a suffix instead of a whole-region change.
        assert_eq!(
            shape.system,
            vec![RegionDigest::of(&request.system.as_ref().unwrap()[0])]
        );
        assert_eq!(shape.config, RegionDigest::ABSENT);
    }

    #[test]
    fn anthropic_thinking_and_tool_choice_are_tracked() {
        let tracker = CacheShapeTracker::default();
        let request = anthropic_request();
        tracker.record("session", CacheShape::from_anthropic(&request));

        let mut forced_tool = request.clone();
        forced_tool.tool_choice = Some(json!({"type":"tool","name":"web_search"}));
        let report = tracker.record("session", CacheShape::from_anthropic(&forced_tool));
        assert_eq!(report.state, PrefixState::ToolsChanged);

        let mut thinking = forced_tool.clone();
        thinking.thinking = Some(json!({"type":"enabled","budget_tokens":1024}));
        let report = tracker.record("session", CacheShape::from_anthropic(&thinking));
        assert_eq!(report.state, PrefixState::ConfigChanged);
    }

    /// Codex appends a developer message such as `<workspace_context>` every
    /// turn. The Anthropic converter lifts those into `system`, so the messages
    /// stay append-only while the system prompt grows.
    fn anthropic_turn(system_blocks: usize, messages: usize) -> MessageRequest {
        let mut request = anthropic_request();
        request.system = Some(
            (0..system_blocks)
                .map(|index| ContentBlock::Text {
                    text: format!("system block {index}"),
                })
                .collect(),
        );
        request.messages = (0..messages)
            .map(|index| Message {
                role: if index % 2 == 0 { "user" } else { "assistant" }.to_owned(),
                content: vec![ContentBlock::Text {
                    text: format!("turn {index}"),
                }],
            })
            .collect();
        request
    }

    /// Remote compaction replaces the transcript with a summary and rebuilds the
    /// system prompt in the same request, so both regions change at once.
    #[test]
    fn compaction_is_named_as_replaced_history_rather_than_instruction_drift() {
        let tracker = CacheShapeTracker::default();
        tracker.record(
            "session",
            CacheShape::from_anthropic(&anthropic_turn(9, 40)),
        );

        let mut compacted = anthropic_turn(1, 1);
        compacted.system = Some(vec![ContentBlock::Text {
            text: "rebuilt system prompt".to_owned(),
        }]);
        compacted.messages = vec![Message {
            role: "user".to_owned(),
            content: vec![ContentBlock::Text {
                text: "summary of the 40 earlier turns".to_owned(),
            }],
        }];
        let report = tracker.record("session", CacheShape::from_anthropic(&compacted));

        assert_eq!(report.state, PrefixState::HistoryTruncated);
        assert_eq!(report.message_prefix_turns, 0);
        assert_eq!(report.previous_turns, 40);
        // The region changes are still reported, they are just not the headline.
        assert!(report.changes.system);
    }

    /// A short session that restarts is not compaction, so it keeps the region
    /// change as its headline.
    #[test]
    fn a_short_restarted_session_is_not_reported_as_compaction() {
        let tracker = CacheShapeTracker::default();
        tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(2, 4)));

        let mut restarted = anthropic_turn(1, 1);
        restarted.system = Some(vec![ContentBlock::Text {
            text: "another system prompt".to_owned(),
        }]);
        restarted.messages = vec![Message {
            role: "user".to_owned(),
            content: vec![ContentBlock::Text {
                text: "unrelated opening turn".to_owned(),
            }],
        }];
        let report = tracker.record("session", CacheShape::from_anthropic(&restarted));

        assert_eq!(report.state, PrefixState::SystemChanged);
    }

    #[test]
    fn a_growing_system_prompt_does_not_hide_a_clean_message_history() {
        let tracker = CacheShapeTracker::default();
        tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(4, 6)));

        // One more system block and two more messages: exactly what a normal
        // Codex turn looks like today.
        let report = tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(5, 8)));

        assert_eq!(report.state, PrefixState::SystemChanged);
        assert!(report.changes.system);
        assert!(!report.changes.tools && !report.changes.config && !report.changes.model);
        assert_eq!(report.changes.as_list(), "system");
        // The provider reuses nothing, because the prompt changed at its start.
        assert_eq!(report.reused_turns, 0);
        assert_eq!(report.reused_bytes, 0);
        // The messages themselves were a clean extension, which is what tells us
        // the system prompt is the culprit rather than the history.
        assert_eq!(report.message_prefix_turns, 6);
        assert_eq!(report.previous_turns, 6);
        assert_eq!(report.total_turns, 8);
        // The first four blocks survived; block five is the new one.
        assert_eq!(report.system_prefix_blocks, 4);
        assert_eq!(report.previous_system_blocks, 4);
        assert_eq!(report.system_blocks, 5);
    }

    #[test]
    fn a_changed_system_prompt_still_reports_a_rewritten_history() {
        let tracker = CacheShapeTracker::default();
        tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(4, 6)));

        let mut rewritten = anthropic_turn(5, 8);
        rewritten.messages[1].content = vec![ContentBlock::Text {
            text: "rewritten history".to_owned(),
        }];
        let report = tracker.record("session", CacheShape::from_anthropic(&rewritten));

        // Two independent problems: the headline is the system prompt, but the
        // message prefix shows the history was also rewritten at index 1.
        assert_eq!(report.state, PrefixState::SystemChanged);
        assert!(report.changes.system);
        assert_eq!(report.message_prefix_turns, 1);
    }

    #[test]
    fn a_stable_system_prompt_with_appended_turns_keeps_the_prefix() {
        let tracker = CacheShapeTracker::default();
        tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(4, 6)));

        let report = tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(4, 8)));

        assert_eq!(report.state, PrefixState::AppendOnly);
        assert_eq!(report.changes.as_list(), "");
        assert_eq!(report.reused_turns, 6);
        assert_eq!(report.message_prefix_turns, 6);
        assert_eq!(report.system_prefix_blocks, 4);
    }

    #[test]
    fn metadata_churn_does_not_look_like_prefix_loss() {
        let tracker = CacheShapeTracker::default();
        let request = anthropic_request();
        tracker.record("session", CacheShape::from_anthropic(&request));
        let mut rerouted = request.clone();
        rerouted.metadata = Some(json!({"session_id":"another-session"}));

        let report = tracker.record("session", CacheShape::from_anthropic(&rerouted));

        assert_eq!(report.state, PrefixState::AppendOnly);
        assert_eq!(report.reused_turns, 1);
    }

    #[test]
    fn absent_region_differs_from_present_empty_region() {
        assert_ne!(RegionDigest::ABSENT, RegionDigest::of(&json!("")));
        assert_ne!(RegionDigest::ABSENT, RegionDigest::of(&json!([])));
    }

    #[test]
    fn identical_requests_produce_identical_shapes() {
        let request = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));

        let first = CacheShape::from_openai_responses(&request);
        let second = CacheShape::from_openai_responses(&request.clone());

        assert_eq!(first, second);
        assert_eq!(first.shape_hash(), second.shape_hash());
    }

    #[test]
    fn first_turn_is_a_cold_start() {
        let request = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));

        let (_, report) = tracker_with_first(&request);

        assert_eq!(report.state, PrefixState::ColdStart);
        assert!(!report.state.is_cache_loss());
        assert_eq!(report.total_turns, 1);
    }

    #[test]
    fn appended_turns_reuse_the_whole_prefix() {
        let first = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));
        let (tracker, _) = tracker_with_first(&first);
        let second = responses_request(
            "You are Codex.",
            json!([turn("say hi"), turn("and again")]),
            json!([]),
        );

        let report = tracker.record("session", CacheShape::from_openai_responses(&second));

        assert_eq!(report.state, PrefixState::AppendOnly);
        assert!(!report.state.is_cache_loss());
        assert_eq!(report.reused_turns, 1);
        assert_eq!(report.total_turns, 2);
        assert!(report.reused_bytes > 0);
    }

    #[test]
    fn rewriting_the_trailing_turn_keeps_the_earlier_prefix() {
        let first = responses_request(
            "You are Codex.",
            json!([turn("say hi"), turn("tool output")]),
            json!([]),
        );
        let (tracker, _) = tracker_with_first(&first);
        let second = responses_request(
            "You are Codex.",
            json!([turn("say hi"), turn("tool output plus more"), turn("next")]),
            json!([]),
        );

        let report = tracker.record("session", CacheShape::from_openai_responses(&second));

        assert_eq!(report.state, PrefixState::TailRewritten);
        assert!(!report.state.is_cache_loss());
        assert_eq!(report.reused_turns, 1);
    }

    #[test]
    fn rewriting_an_earlier_turn_breaks_the_prefix() {
        let first = responses_request(
            "You are Codex.",
            json!([turn("say hi"), turn("second"), turn("third")]),
            json!([]),
        );
        let (tracker, _) = tracker_with_first(&first);
        let second = responses_request(
            "You are Codex.",
            json!([
                turn("say hi"),
                turn("rewritten"),
                turn("third"),
                turn("fourth")
            ]),
            json!([]),
        );

        let report = tracker.record("session", CacheShape::from_openai_responses(&second));

        assert_eq!(report.state, PrefixState::TurnRewritten);
        assert!(report.state.is_cache_loss());
        assert_eq!(report.reused_turns, 1);
    }

    #[test]
    fn dropping_history_reports_truncation() {
        let first = responses_request(
            "You are Codex.",
            json!([turn("say hi"), turn("second"), turn("third")]),
            json!([]),
        );
        let (tracker, _) = tracker_with_first(&first);
        let second = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));

        let report = tracker.record("session", CacheShape::from_openai_responses(&second));

        assert_eq!(report.state, PrefixState::HistoryTruncated);
        assert!(report.state.is_cache_loss());
        assert_eq!(report.previous_turns, 3);
        assert_eq!(report.total_turns, 1);
    }

    #[test]
    fn instructions_and_tool_drift_are_reported_separately() {
        let first = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));
        let (tracker, _) = tracker_with_first(&first);

        let changed_system =
            responses_request("You are someone else.", json!([turn("say hi")]), json!([]));
        let report = tracker.record(
            "session",
            CacheShape::from_openai_responses(&changed_system),
        );
        assert_eq!(report.state, PrefixState::SystemChanged);
        assert_eq!(report.reused_turns, 0);

        let changed_tools = responses_request(
            "You are someone else.",
            json!([turn("say hi")]),
            json!([{"type":"function","name":"exec_command"}]),
        );
        let report = tracker.record("session", CacheShape::from_openai_responses(&changed_tools));
        assert_eq!(report.state, PrefixState::ToolsChanged);
    }

    #[test]
    fn reordering_tools_breaks_the_prefix() {
        let ordered = responses_request(
            "You are Codex.",
            json!([turn("say hi")]),
            json!([{"name":"a"},{"name":"b"}]),
        );
        let (tracker, _) = tracker_with_first(&ordered);
        let reordered = responses_request(
            "You are Codex.",
            json!([turn("say hi")]),
            json!([{"name":"b"},{"name":"a"}]),
        );

        let report = tracker.record("session", CacheShape::from_openai_responses(&reordered));

        assert_eq!(report.state, PrefixState::ToolsChanged);
    }

    #[test]
    fn reasoning_config_drift_is_reported() {
        let mut first = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));
        first["reasoning"] = json!({"effort":"medium"});
        let (tracker, _) = tracker_with_first(&first);
        let mut second = first.clone();
        second["reasoning"] = json!({"effort":"high"});

        let report = tracker.record("session", CacheShape::from_openai_responses(&second));

        assert_eq!(report.state, PrefixState::ConfigChanged);
    }

    #[test]
    fn switching_protocol_or_model_resets_the_prefix() {
        let request = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));
        let (tracker, _) = tracker_with_first(&request);

        let chat = json!({
            "model": "provider-model",
            "messages": [{"role":"system","content":"You are Codex."}]
        });
        let report = tracker.record("session", CacheShape::from_openai_chat(&chat));
        assert_eq!(report.state, PrefixState::ProtocolChanged);

        let mut other_model = chat.clone();
        other_model["model"] = json!("another-model");
        let report = tracker.record("session", CacheShape::from_openai_chat(&other_model));
        assert_eq!(report.state, PrefixState::ModelChanged);
    }

    #[test]
    fn separate_sessions_do_not_interfere() {
        let first = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));
        let (tracker, _) = tracker_with_first(&first);
        let other = responses_request("You are Codex.", json!([turn("different")]), json!([]));

        let report = tracker.record("other-session", CacheShape::from_openai_responses(&other));

        assert_eq!(report.state, PrefixState::ColdStart);
    }

    #[test]
    fn interleaved_prompts_in_one_session_are_tracked_separately() {
        // Fusion roles and concurrent subagents share a session id while sending
        // unrelated prompts. Each lineage has to keep its own prefix.
        let tracker = CacheShapeTracker::default();
        let panel = responses_request("Panel prompt.", json!([turn("say hi")]), json!([]));
        let judge = responses_request("Judge prompt.", json!([turn("judge this")]), json!([]));
        tracker.record("session", CacheShape::from_openai_responses(&panel));
        tracker.record("session", CacheShape::from_openai_responses(&judge));

        let panel_next = responses_request(
            "Panel prompt.",
            json!([turn("say hi"), turn("more")]),
            json!([]),
        );
        let report = tracker.record("session", CacheShape::from_openai_responses(&panel_next));
        assert_eq!(report.state, PrefixState::AppendOnly);
        assert_eq!(report.reused_turns, 1);

        let judge_next = responses_request(
            "Judge prompt.",
            json!([turn("judge this"), turn("again")]),
            json!([]),
        );
        let report = tracker.record("session", CacheShape::from_openai_responses(&judge_next));
        assert_eq!(report.state, PrefixState::AppendOnly);
        assert_eq!(report.reused_turns, 1);
    }

    #[test]
    fn only_the_newest_lineages_are_remembered_per_session() {
        let tracker = CacheShapeTracker::default();

        for index in 0..TRACKED_SHAPES_PER_SESSION + 3 {
            let request = responses_request(
                &format!("prompt {index}"),
                json!([turn("say hi")]),
                json!([]),
            );
            tracker.record("session", CacheShape::from_openai_responses(&request));
        }

        let sessions = tracker.sessions.lock().unwrap();
        assert_eq!(
            sessions.entries["session"].shapes.len(),
            TRACKED_SHAPES_PER_SESSION
        );
    }

    #[test]
    fn tracking_is_bounded_by_session_capacity() {
        let tracker = CacheShapeTracker::default();
        let request = responses_request("You are Codex.", json!([turn("say hi")]), json!([]));

        for index in 0..TRACKED_SESSIONS + 8 {
            tracker.record(
                &format!("session-{index}"),
                CacheShape::from_openai_responses(&request),
            );
        }

        let sessions = tracker.sessions.lock().unwrap();
        assert_eq!(sessions.entries.len(), TRACKED_SESSIONS);
        assert!(!sessions.entries.contains_key("session-0"));
        assert!(
            sessions
                .entries
                .contains_key(&format!("session-{}", TRACKED_SESSIONS + 7))
        );
    }

    #[test]
    fn chat_shape_tracks_the_leading_system_message() {
        let first = json!({
            "model": "provider-model",
            "messages": [
                {"role":"system","content":"You are Codex."},
                {"role":"user","content":"say hi"}
            ]
        });
        let tracker = CacheShapeTracker::default();
        tracker.record("session", CacheShape::from_openai_chat(&first));
        let mut second = first.clone();
        second["messages"][0]["content"] = json!("You are someone else.");

        let report = tracker.record("session", CacheShape::from_openai_chat(&second));

        assert_eq!(report.state, PrefixState::SystemChanged);
    }

    fn observation(state: PrefixState, stable_prefix_bytes: usize) -> PrefixObservation {
        // Whole request reusable, which is what a normal appended turn looks like.
        observation_with_total(state, stable_prefix_bytes, stable_prefix_bytes.max(1))
    }

    fn observation_with_total(
        state: PrefixState,
        stable_prefix_bytes: usize,
        total_bytes: usize,
    ) -> PrefixObservation {
        PrefixObservation {
            provider_id: "baidu-oneapi".to_owned(),
            catalog_slug: "gpt-5.6-sol-baidu-oneapi".to_owned(),
            upstream_model_id: "gpt-5.6-sol".to_owned(),
            protocol: ANTHROPIC_MESSAGES,
            state,
            changed_regions: String::new(),
            stable_prefix_bytes,
            total_bytes,
            reused_turns: 40,
            total_turns: 41,
            usage: Arc::new(TokenUsageAggregator::default()),
        }
    }

    /// A turn that appends a large tool result can only reuse part of its prompt,
    /// so the verdict has to compare against the reusable part rather than the
    /// whole prompt. This is the shape that produced 182 false findings on a
    /// grok-4.5 session.
    #[test]
    fn a_turn_with_a_large_appended_tail_is_judged_against_its_reusable_prefix() {
        // Roughly half the request is the freshly appended tail.
        let subject = observation_with_total(PrefixState::AppendOnly, 74_817, 159_000);
        let healthy = UpstreamCacheUsage {
            input_tokens: Some(21_815),
            cache_read_tokens: Some(17_792),
            cache_creation_tokens: None,
            output_tokens: None,
        };

        // 45% of the prompt, but essentially all of what could be reused.
        assert!(!subject.discarded_by_provider(&healthy));

        let dropped = UpstreamCacheUsage {
            input_tokens: Some(39_479),
            cache_read_tokens: Some(128),
            cache_creation_tokens: None,
            output_tokens: None,
        };
        assert!(subject.discarded_by_provider(&dropped));
    }

    #[test]
    fn a_provider_that_drops_a_preserved_prefix_is_separated_from_our_own_cache_loss() {
        let usage = UpstreamCacheUsage {
            input_tokens: Some(95_744),
            cache_read_tokens: Some(3_456),
            cache_creation_tokens: None,
            output_tokens: None,
        };

        // Prefix kept byte-identical, provider still recomputed it.
        assert!(observation(PrefixState::AppendOnly, 960_000).discarded_by_provider(&usage));
        // Our own prefix loss already has its own warning, so this stays quiet.
        assert!(!observation(PrefixState::SystemChanged, 960_000).discarded_by_provider(&usage));
        // A cold start had no prefix to reuse in the first place.
        assert!(!observation(PrefixState::ColdStart, 0).discarded_by_provider(&usage));
        // A provider that omits the counters entirely cannot be judged.
        assert!(
            !observation(PrefixState::AppendOnly, 960_000).discarded_by_provider(
                &UpstreamCacheUsage {
                    input_tokens: Some(99_269),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    output_tokens: None,
                }
            )
        );
        // A real hit is not a finding. This shape used to be reported as a miss
        // because a bytes-per-token estimate inflated the expected prefix.
        assert!(
            !observation(PrefixState::AppendOnly, 960_000).discarded_by_provider(
                &UpstreamCacheUsage {
                    input_tokens: Some(1_391),
                    cache_read_tokens: Some(92_544),
                    cache_creation_tokens: None,
                    output_tokens: None,
                }
            )
        );
    }

    /// The system prompt and tool definitions sit ahead of the message list, so a
    /// provider cache hit has to be judged against them too.
    #[test]
    fn the_stable_prefix_spans_system_and_tools_not_only_replayed_turns() {
        let tracker = CacheShapeTracker::default();
        let first = anthropic_turn(4, 6);
        tracker.record("session", CacheShape::from_anthropic(&first));

        let report = tracker.record("session", CacheShape::from_anthropic(&anthropic_turn(4, 8)));

        assert_eq!(report.state, PrefixState::AppendOnly);
        assert!(report.stable_prefix_bytes > report.reused_bytes);

        // A changed region invalidates from the first token, so nothing is stable.
        let mut different_system = anthropic_turn(4, 8);
        different_system.system = Some(vec![ContentBlock::Text {
            text: "another system prompt".to_owned(),
        }]);
        let report = tracker.record("session", CacheShape::from_anthropic(&different_system));

        assert!(report.state.is_cache_loss());
        assert_eq!(report.stable_prefix_bytes, 0);
    }

    #[test]
    fn provider_cache_verdicts_ignore_prefixes_too_small_to_cache() {
        let prompt_of = |prompt_tokens: u64| UpstreamCacheUsage {
            input_tokens: Some(prompt_tokens - 16),
            cache_read_tokens: Some(16),
            cache_creation_tokens: None,
            output_tokens: None,
        };

        let subject = observation(PrefixState::AppendOnly, 960_000);
        assert!(
            !subject.discarded_by_provider(&prompt_of(MIN_PROMPT_TOKENS_FOR_VERDICT - 1)),
            "a prompt below the cacheable floor says nothing about the shape"
        );
        assert!(subject.discarded_by_provider(&prompt_of(MIN_PROMPT_TOKENS_FOR_VERDICT)));
    }

    #[tokio::test]
    async fn cache_usage_observation_relays_bytes_untouched() {
        let events = vec![
            Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":240000,\"cache_read_input_tokens\":3456}}}\n\n",
            ),
            Bytes::from_static(
                b"event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":46}}\n\n",
            ),
        ];
        let upstream = futures_util::stream::iter(events.clone().into_iter().map(Ok));

        let relayed: Vec<Bytes> = observe_upstream_cache_usage(
            upstream,
            Some(observation(PrefixState::AppendOnly, 960_000)),
        )
        .map(Result::unwrap)
        .collect()
        .await;

        assert_eq!(relayed, events);
    }

    #[tokio::test]
    async fn cache_usage_observation_reads_responses_completed_usage() {
        let events = vec![
            Bytes::from_static(
                b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":4,\"input_tokens_details\":{\"cached_tokens\":3},\"output_tokens\":2}}}\n\n",
            ),
        ];
        let usage = Arc::new(TokenUsageAggregator::default());
        let upstream = futures_util::stream::iter(events.clone().into_iter().map(Ok));
        let mut observation = observation(PrefixState::AppendOnly, 960_000);
        observation.usage = usage.clone();

        let relayed: Vec<Bytes> = observe_upstream_cache_usage(upstream, Some(observation))
            .map(Result::unwrap)
            .collect()
            .await;

        assert_eq!(relayed, events);
        let snapshot = usage.snapshot();
        assert_eq!(snapshot[0].input_tokens, 1);
        assert_eq!(snapshot[0].cache_read_tokens, 3);
        assert_eq!(snapshot[0].output_tokens, 2);
    }

    #[test]
    fn usage_counters_are_absorbed_from_both_message_and_top_level_shapes() {
        let mut usage = UpstreamCacheUsage::default();
        // Anthropic opens with a zeroed `usage`, which must not count as observed.
        usage.absorb(&json!({"input_tokens": 0, "output_tokens": 0}));
        assert!(!usage.observed());

        usage.absorb(&json!({"input_tokens": 100, "cache_read_input_tokens": 3456}));
        usage.absorb(&json!({"cache_creation_input_tokens": 7}));
        usage.absorb(&json!({"output_tokens": 42}));

        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.cache_read_tokens, Some(3456));
        assert_eq!(usage.cache_creation_tokens, Some(7));
        assert_eq!(usage.output_tokens, Some(42));
        assert!(usage.observed());
    }

    /// Baidu OneAPI reports an optimistic partial count on `message_start` and
    /// the real one on `message_delta`. Only the last frame is the verdict.
    #[test]
    fn a_later_usage_frame_replaces_an_earlier_partial_one() {
        let mut usage = UpstreamCacheUsage::default();
        usage.absorb(&json!({"input_tokens": 15_744, "cache_read_input_tokens": 123_021}));
        let subject = observation(PrefixState::AppendOnly, 960_000);
        assert!(
            !subject.discarded_by_provider(&usage),
            "the partial frame looks like a hit"
        );

        usage.absorb(&json!({"input_tokens": 135_947, "cache_read_input_tokens": 3_456}));

        assert_eq!(usage.cache_read_tokens, Some(3_456));
        assert_eq!(usage.input_tokens, Some(135_947));
        assert!(
            subject.discarded_by_provider(&usage),
            "the final frame is the verdict"
        );
    }

    #[test]
    fn token_usage_aggregator_sums_provider_counters_and_cache_hit_rate() {
        let aggregator = TokenUsageAggregator::default();
        aggregator.record(
            "baidu-oneapi",
            "gpt-5.6-sol",
            &UpstreamCacheUsage {
                input_tokens: Some(1_000),
                cache_read_tokens: Some(3_000),
                cache_creation_tokens: Some(500),
                output_tokens: Some(200),
            },
        );
        aggregator.record(
            "baidu-oneapi",
            "gpt-5.6-sol",
            &UpstreamCacheUsage {
                input_tokens: Some(500),
                cache_read_tokens: Some(1_500),
                cache_creation_tokens: None,
                output_tokens: Some(100),
            },
        );

        let snapshot = aggregator.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].provider_id, "baidu-oneapi");
        assert_eq!(snapshot[0].model_id, "gpt-5.6-sol");
        assert_eq!(snapshot[0].request_count, 2);
        assert_eq!(snapshot[0].input_tokens, 1_500);
        assert_eq!(snapshot[0].cache_read_tokens, 4_500);
        assert_eq!(snapshot[0].cache_creation_tokens, 500);
        assert_eq!(snapshot[0].output_tokens, 300);
        assert_eq!(
            snapshot[0].cache_hit_percent,
            Some(4_500.0 / 6_500.0 * 100.0)
        );
        let daily_snapshot = aggregator.snapshot_for_days(1).unwrap();
        assert_eq!(daily_snapshot.len(), 1);
        assert_eq!(daily_snapshot[0].request_count, 2);
        assert_eq!(daily_snapshot[0].input_tokens, 1_500);

        aggregator.record(
            "baidu-oneapi",
            "DeepSeek-V4-Flash",
            &UpstreamCacheUsage {
                input_tokens: Some(50),
                cache_read_tokens: Some(150),
                cache_creation_tokens: None,
                output_tokens: Some(25),
            },
        );
        let snapshot = aggregator.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].model_id, "DeepSeek-V4-Flash");
        assert_eq!(snapshot[0].request_count, 1);
        assert_eq!(snapshot[1].model_id, "gpt-5.6-sol");
        assert_eq!(snapshot[1].request_count, 2);
    }

    /// Chat Completions upstreams report cache hits under their own names, and
    /// `prompt_tokens` includes them, so the uncached part has to be derived.
    #[test]
    fn chat_and_responses_cache_counters_normalise_to_uncached_input() {
        let mut deepseek = UpstreamCacheUsage::default();
        deepseek.absorb(
            &json!({"prompt_tokens": 12_000, "prompt_cache_hit_tokens": 11_500, "prompt_cache_miss_tokens": 500}),
        );
        assert_eq!(deepseek.cache_read_tokens, Some(11_500));
        assert_eq!(deepseek.input_tokens, Some(500));

        let mut chat = UpstreamCacheUsage::default();
        chat.absorb(
            &json!({"prompt_tokens": 12_000, "prompt_tokens_details": {"cached_tokens": 9_000}}),
        );
        assert_eq!(chat.cache_read_tokens, Some(9_000));
        assert_eq!(chat.input_tokens, Some(3_000));

        let mut responses = UpstreamCacheUsage::default();
        responses.absorb(
            &json!({"input_tokens": 4_000, "input_tokens_details": {"cached_tokens": 3_500}}),
        );
        assert_eq!(responses.cache_read_tokens, Some(3_500));
        assert_eq!(responses.input_tokens, Some(500));
    }

    #[test]
    fn official_observer_records_responses_token_and_cache_usage() {
        let tracker = CacheShapeTracker::with_usage(TokenUsageAggregator::default());
        let request = json!({
            "model": "gpt-5.6-sol",
            "instructions": "stable",
            "tools": [],
            "input": [{"type":"message","role":"user","content":"hello"}]
        });
        let routing = UpstreamRouting {
            session_id: "thread-1".to_owned(),
            hash_key: "hash-1".to_owned(),
        };
        let observation = record_provider_prefix(
            &tracker,
            "official",
            "gpt-5.6-sol",
            "gpt-5.6-sol",
            Some(&routing),
            CacheShape::from_openai_responses(&request),
        )
        .unwrap();
        let mut observer = UpstreamCacheObserver::new(observation);
        observer.observe_value(&json!({
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 120,
                    "input_tokens_details": {"cached_tokens": 80},
                    "output_tokens": 30
                }
            }
        }));
        drop(observer);

        let usage = tracker.usage_snapshot();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].provider_id, "official");
        assert_eq!(usage[0].model_id, "gpt-5.6-sol");
        assert_eq!(usage[0].input_tokens, 40);
        assert_eq!(usage[0].cache_read_tokens, 80);
        assert_eq!(usage[0].output_tokens, 30);
    }
}
