use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use super::checkpoint;
use super::guidance::{GUIDANCE, MIN_RANGE_CHARS, PendingTier};
use super::store::{SegmentRow, Store, TurnSample};
use super::tools;
use super::types::{
    CompressibleRange, ContextStats, ContextStatus, DirectRef, Protection, SegmentKind,
    SessionStats, SessionSummary, StatsTotals, Tier, TurnView, estimate_item_tokens,
    estimate_text_tokens, is_context_tool, session_display_name,
};

/// A turn is serialized against other turns of the same session, so a lease
/// must outlive one model exchange but not leak after a crash. The TTL only
/// guards against a holder that died without releasing.
pub(crate) const LEASE_TTL_SECONDS: i64 = 30 * 60;

pub struct ContextEngine {
    store: Mutex<Store>,
    settings: crate::config::ContextSettings,
}

/// Held for the whole turn; the session becomes usable again on drop.
pub struct Lease {
    engine: std::sync::Arc<ContextEngine>,
    session: String,
    owner: String,
}

impl Drop for Lease {
    fn drop(&mut self) {
        match self.engine.lock() {
            Ok(mut store) => {
                if let Err(error) = store.release_lease(&self.session, &self.owner) {
                    tracing::warn!(error = %error, "failed to release context lease");
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "failed to lock the context store to release a lease");
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolExecution {
    pub content: String,
    pub changed: bool,
    pub revision: i64,
}

impl ContextEngine {
    pub fn open(settings: crate::config::ContextSettings, path: &Path) -> Result<Self> {
        settings.validate()?;
        let store = Store::open(path)?;
        Ok(Self {
            store: Mutex::new(store),
            settings,
        })
    }

    #[cfg(test)]
    pub fn open_in_memory(settings: crate::config::ContextSettings) -> Result<Self> {
        let store = Store::open_in_memory()?;
        Ok(Self {
            store: Mutex::new(store),
            settings,
        })
    }

    pub fn settings(&self) -> &crate::config::ContextSettings {
        &self.settings
    }

    pub fn is_enabled(&self) -> bool {
        self.settings.enabled
    }

    pub fn tool_definitions(&self) -> Vec<Value> {
        tools::tool_definitions()
    }

    pub fn guidance(&self) -> &'static str {
        GUIDANCE
    }

    pub fn acquire_lease(self: &std::sync::Arc<Self>, session: &str) -> Result<Lease> {
        let owner = uuid::Uuid::new_v4().to_string();
        self.lock()?
            .acquire_lease(session, &owner, LEASE_TTL_SECONDS)?;
        Ok(Lease {
            engine: std::sync::Arc::clone(self),
            session: session.to_owned(),
            owner,
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Store>> {
        self.store
            .lock()
            .map_err(|_| anyhow!("context store lock is poisoned"))
    }

    pub fn begin_turn(
        &self,
        session: &str,
        items: &[Value],
        window_tokens: u64,
    ) -> Result<TurnView> {
        let fingerprints: Vec<String> = items.iter().map(fingerprint).collect();
        let encoded: Vec<String> = items
            .iter()
            .map(|item| serde_json::to_string(item).context("encode history item"))
            .collect::<Result<_>>()?;
        let mut store = self.lock()?;
        let (session_row, _) = store.sync_history(session, &fingerprints, &encoded)?;
        let segments = store.segments(session, session_row.revision)?;
        let rendered = render_view(&store, session, &segments)?;
        let protection = compute_protection(&store, session, &segments, &self.settings)?;
        let mut estimated = 0u64;
        for item in &rendered {
            estimated = estimated.saturating_add(estimate_item_tokens(item));
        }
        estimated = estimated.saturating_add(estimate_text_tokens(GUIDANCE));
        Ok(TurnView {
            revision: session_row.revision,
            items: rendered,
            estimated_tokens: estimated,
            window_tokens,
            protection,
        })
    }

    /// Replaces a local compaction checkpoint with the archived history plus
    /// whatever the client sent after it. Returns None when the input carries
    /// no checkpoint, so callers keep the existing flow.
    pub fn expand_checkpoint(
        &self,
        session: &str,
        items: &[Value],
        secret: &str,
    ) -> Result<Option<Vec<Value>>> {
        let Some(index) = items.iter().rposition(checkpoint::is_checkpoint_item) else {
            return Ok(None);
        };
        let token = items[index]
            .get("encrypted_content")
            .and_then(Value::as_str)
            .context("context checkpoint item has no token")?;
        let decoded =
            checkpoint::decode(token, secret).map_err(|error| anyhow!(error.to_string()))?;
        if decoded.session != session {
            bail!(
                "compaction checkpoint belongs to context session {}",
                crate::context::types::session_display_name(&decoded.session)
            );
        }
        let mut store = self.lock()?;
        let stored = store.ensure_session(session)?;
        if decoded.revision > stored.revision {
            bail!(
                "compaction checkpoint revision {} is newer than the archived session revision {}",
                decoded.revision,
                stored.revision
            );
        }
        let mut expanded = store.history_items(session)?;
        drop(store);
        if expanded.is_empty() {
            bail!(
                "context session {} has no archived history",
                crate::context::types::session_display_name(session)
            );
        }
        expanded.extend(items[index + 1..].iter().cloned());
        Ok(Some(expanded))
    }

    /// Rebuilds the full input for a chained request that carries only a delta.
    ///
    /// The archived originals plus the recorded output of the previous
    /// response are the client-visible conversation, so an upstream call
    /// always restarts from the full view instead of a stale response id. A
    /// client that resends its own history is left untouched.
    pub fn expand_chained_input(
        &self,
        session: &str,
        response_id: &str,
        incoming: &[Value],
    ) -> Result<Vec<Value>> {
        let store = self.lock()?;
        let Some(recorded) = store.response_output(session, response_id)? else {
            if let Some(owner) = store.response_owner(response_id)? {
                bail!(
                    "response id {response_id} belongs to context session {}",
                    crate::context::types::session_display_name(&owner)
                );
            }
            if incoming.is_empty() {
                bail!("unknown response id {response_id}");
            }
            tracing::warn!(
                response_id,
                "context engine rebuilds an unknown response id from the client history"
            );
            return Ok(incoming.to_vec());
        };
        let mut full = store.history_items(session)?;
        full.extend(recorded);
        let overlap = common_prefix_length(&full, incoming);
        if overlap == full.len() && incoming.len() >= full.len() {
            return Ok(incoming.to_vec());
        }
        if incoming.is_empty() || overlap == 0 || overlap == incoming.len() {
            full.extend_from_slice(incoming);
            return Ok(full);
        }
        tracing::warn!(
            response_id,
            overlap,
            "context engine trusts a divergent client history"
        );
        Ok(incoming.to_vec())
    }

    /// Commits the response mapping a later chained request relies on.
    pub fn record_response(
        &self,
        session: &str,
        response_id: &str,
        revision: i64,
        output: &[Value],
    ) -> Result<()> {
        let mut store = self.lock()?;
        store.record_response(session, response_id, revision, output)
    }

    pub fn execute_tool(
        &self,
        session: &str,
        revision: i64,
        name: &str,
        arguments: &Value,
        window_tokens: u64,
    ) -> Result<ToolExecution> {
        if !is_context_tool(name) {
            bail!("unknown context tool {name}");
        }
        let mut store = self.lock()?;
        let session_row = store.ensure_session(session)?;
        if session_row.revision != revision {
            bail!(
                "context revision changed from {revision} to {}",
                session_row.revision
            );
        }
        let segments = store.segments(session, revision)?;
        match name {
            super::types::TOOL_COMPRESS => {
                let (block_id, new_revision) = compress(
                    &mut store,
                    session,
                    revision,
                    &segments,
                    &self.settings,
                    arguments,
                )?;
                Ok(ToolExecution {
                    content: json!({"ok": true, "block_id": block_id, "revision": new_revision})
                        .to_string(),
                    changed: true,
                    revision: new_revision,
                })
            }
            super::types::TOOL_DECOMPRESS => {
                let block_id = arguments
                    .get("block_id")
                    .and_then(Value::as_str)
                    .context("decompress requires block_id")?;
                let apply = arguments
                    .get("apply")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let block = store
                    .block(session, block_id)?
                    .with_context(|| format!("block {block_id} does not exist"))?;
                if !apply {
                    return Ok(ToolExecution {
                        content: json!({
                            "ok": true,
                            "block_id": block.id,
                            "tier": block.tier.as_u8(),
                            "topic": block.topic,
                            "direct_refs": block.direct_refs.iter().map(|entry| entry.reference.clone()).collect::<Vec<_>>(),
                            "full_refs": block.full_refs,
                        })
                        .to_string(),
                        changed: false,
                        revision,
                    });
                }
                let new_revision = store.restore_block(session, revision, block_id)?;
                Ok(ToolExecution {
                    content: json!({"ok": true, "block_id": block_id, "revision": new_revision})
                        .to_string(),
                    changed: true,
                    revision: new_revision,
                })
            }
            super::types::TOOL_SEARCH => {
                let query = arguments
                    .get("query")
                    .and_then(Value::as_str)
                    .context("search requires query")?;
                let limit = arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(5)
                    .clamp(1, 20) as usize;
                let hits = store.search(session, query, limit)?;
                Ok(ToolExecution {
                    content: json!({
                        "ok": true,
                        "hits": hits.iter().map(|hit| json!({
                            "kind": hit.kind,
                            "reference": hit.reference,
                            "title": hit.title,
                            "snippet": hit.snippet,
                        })).collect::<Vec<_>>(),
                    })
                    .to_string(),
                    changed: false,
                    revision,
                })
            }
            super::types::TOOL_STATUS => {
                let status = status(&store, session, revision, window_tokens, &self.settings)?;
                Ok(ToolExecution {
                    content: serde_json::to_string(&status).context("encode context status")?,
                    changed: false,
                    revision,
                })
            }
            other => bail!("unknown context tool {other}"),
        }
    }

    pub fn status(
        &self,
        session: &str,
        window_tokens: u64,
        enabled: bool,
    ) -> Result<ContextStatus> {
        let mut store = self.lock()?;
        let session_row = store.ensure_session(session)?;
        let mut status = status(
            &store,
            session,
            session_row.revision,
            window_tokens,
            &self.settings,
        )?;
        status.enabled = enabled;
        Ok(status)
    }

    /// Builds and stamps the upstream-compatible nudge under the session lease.
    pub(crate) fn pressure_note(
        &self,
        session: &str,
        view: &TurnView,
        advise: bool,
    ) -> Result<String> {
        let store = self.lock()?;
        let segments = store.segments(session, view.revision)?;
        let ranges = compressible_ranges(
            &store,
            session,
            &segments,
            &view.protection,
            self.settings.max_tier,
        )?;
        let mut tiers: [PendingTier; 3] = std::array::from_fn(|_| PendingTier::default());
        let positions: HashMap<&str, usize> = segments
            .iter()
            .enumerate()
            .map(|(index, segment)| (segment.reference.as_str(), index))
            .collect();
        for range in ranges {
            let tier = match range.tier.as_str() {
                "t1" => 0,
                "t2" => 1,
                "t3" => 2,
                _ => bail!("invalid compression candidate tier {}", range.tier),
            };
            // Match the kernel minCompressRange gate on actual message text,
            // not JSON framing or rounded-up token estimates.
            if tier == 0 {
                let mut chars = 0;
                let start = positions
                    .get(range.start.as_str())
                    .context("candidate start is missing")?;
                let end = positions
                    .get(range.end.as_str())
                    .context("candidate end is missing")?;
                for segment in &segments[*start..=*end] {
                    let raw = store
                        .message_json(session, &segment.reference)?
                        .with_context(|| {
                            format!("archived message {} is missing", segment.reference)
                        })?;
                    let item: Value =
                        serde_json::from_str(&raw).context("decode archived message")?;
                    chars += message_chars(&item);
                }
                if chars < MIN_RANGE_CHARS {
                    continue;
                }
                tiers[tier].tokens = tiers[tier].tokens.saturating_add(range.estimated_tokens);
            }
            if tiers[tier].oldest.is_none() {
                tiers[tier].oldest = Some((range.start, range.end));
            }
        }
        // Distillation mass counts active summaries, without topic/framing
        // overhead, as pendingByTier does in the upstream kernel.
        for segment in segments
            .iter()
            .filter(|segment| segment.kind == SegmentKind::Block)
        {
            let block = store
                .block(session, &segment.reference)?
                .with_context(|| format!("context block {} is missing", segment.reference))?;
            let tier = usize::from(block.tier.as_u8());
            if tier < 3 {
                tiers[tier].tokens = tiers[tier]
                    .tokens
                    .saturating_add(estimate_text_tokens(&block.summary));
                tiers[tier].blocks += 1;
            }
        }
        // Host ranges must be executable: a protected or isolated block is
        // not an actionable distillation target in this contiguous adapter.
        for tier in &mut tiers {
            if tier.oldest.is_none() {
                tier.tokens = 0;
                tier.blocks = 0;
            }
        }
        let mut state = store.nudge_state(session)?;
        let note = state.note(
            view.estimated_tokens,
            view.window_tokens,
            &self.settings,
            &tiers,
            advise,
        );
        if advise {
            store.save_nudge_state(session, &state)?;
        }
        Ok(note)
    }

    pub fn sessions(&self) -> Result<Vec<SessionSummary>> {
        let store = self.lock()?;
        store.session_summaries()
    }

    /// Records the billed usage of one upstream round so the console can show
    /// the cache hit rate of a context session.
    pub(crate) fn record_turn(&self, session: &str, sample: TurnSample) -> Result<()> {
        self.lock()?
            .record_turn(session, sample)
            .with_context(|| format!("record turn usage for {}", session_display_name(session)))
    }

    /// Compression and cache totals per session for the console page.
    pub fn stats(&self, window_tokens: u64, enabled: bool) -> Result<ContextStats> {
        let usage = self.lock()?.session_usage()?;
        let mut sessions = Vec::with_capacity(usage.len());
        for row in usage {
            let status = self.status(&row.session, window_tokens, enabled)?;
            sessions.push(SessionStats {
                display: session_display_name(&row.session),
                session: row.session,
                revision: row.revision,
                updated_at: row.updated_at,
                messages: row.messages,
                compressions: row.compressions,
                saved_tokens: row.saved_tokens,
                view_tokens: status.estimated_view_tokens,
                tiers: status.tiers,
                cache: row.cache,
            });
        }
        let mut totals = StatsTotals {
            sessions: sessions.len() as u64,
            ..StatsTotals::default()
        };
        for session in &sessions {
            totals.messages = totals.messages.saturating_add(session.messages);
            totals.compressions = totals.compressions.saturating_add(session.compressions);
            totals.saved_tokens = totals.saved_tokens.saturating_add(session.saved_tokens);
            totals.view_tokens = totals.view_tokens.saturating_add(session.view_tokens);
            totals.cache.merge(&session.cache);
        }
        Ok(ContextStats {
            enabled,
            sessions,
            totals,
        })
    }

    pub fn export(&self, session: &str) -> Result<Value> {
        let store = self.lock()?;
        store.export(session)
    }
}

fn common_prefix_length(left: &[Value], right: &[Value]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| fingerprint(left) == fingerprint(right))
        .count()
}

fn fingerprint(item: &Value) -> String {
    let mut value = item.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("id");
        object.remove("status");
    }
    value.to_string()
}
fn render_view(store: &Store, session: &str, segments: &[SegmentRow]) -> Result<Vec<Value>> {
    let mut items = Vec::new();
    for segment in segments {
        match segment.kind {
            SegmentKind::Message => {
                let raw = store
                    .message_json(session, &segment.reference)?
                    .with_context(|| {
                        format!("archived message {} is missing", segment.reference)
                    })?;
                let item: Value = serde_json::from_str(&raw).context("decode archived message")?;
                items.push(item);
            }
            SegmentKind::Block => {
                let block = store
                    .block(session, &segment.reference)?
                    .with_context(|| format!("context block {} is missing", segment.reference))?;
                let refs = block
                    .direct_refs
                    .iter()
                    .map(|entry| entry.reference.clone())
                    .collect::<Vec<_>>()
                    .join(",");
                let text = format!(
                    "<context_summary id=\"{}\" tier=\"{}\" refs=\"{}\">\n{}\n{}\n</context_summary>",
                    block.id,
                    block.tier.as_u8(),
                    refs,
                    block.topic,
                    block.summary
                );
                items.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": text}],
                }));
            }
        }
    }
    Ok(items)
}

fn compute_protection(
    store: &Store,
    session: &str,
    segments: &[SegmentRow],
    settings: &crate::config::ContextSettings,
) -> Result<Protection> {
    let mut items: Vec<Option<Value>> = Vec::with_capacity(segments.len());
    for segment in segments {
        match segment.kind {
            SegmentKind::Message => {
                let raw = store
                    .message_json(session, &segment.reference)?
                    .with_context(|| {
                        format!("archived message {} is missing", segment.reference)
                    })?;
                let item: Value = serde_json::from_str(&raw).context("decode archived message")?;
                items.push(Some(item));
            }
            SegmentKind::Block => items.push(None),
        }
    }
    let mut protected: HashMap<usize, String> = HashMap::new();
    let role_of = |item: &Option<Value>| -> Option<String> {
        item.as_ref()
            .and_then(|value| value.get("role"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    for (index, item) in items.iter().enumerate() {
        if matches!(role_of(item).as_deref(), Some("system" | "developer")) {
            protected
                .entry(index)
                .or_insert_with(|| "system instructions".to_owned());
        }
    }
    let user_indices: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| role_of(item).as_deref() == Some("user"))
        .map(|(index, _)| index)
        .collect();
    if let Some(first) = user_indices.first() {
        protected
            .entry(*first)
            .or_insert_with(|| "first user message".to_owned());
    }
    if let Some(last) = user_indices.last() {
        protected
            .entry(*last)
            .or_insert_with(|| "latest user message".to_owned());
    }
    let message_indices: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.is_some())
        .map(|(index, _)| index)
        .collect();
    for index in message_indices
        .iter()
        .rev()
        .take(settings.protected_messages)
    {
        protected
            .entry(*index)
            .or_insert_with(|| "recent message".to_owned());
    }
    let mut accumulated = 0u64;
    for index in message_indices.iter().rev() {
        if accumulated >= settings.protected_tokens {
            break;
        }
        protected
            .entry(*index)
            .or_insert_with(|| "recent token window".to_owned());
        if let Some(item) = &items[*index] {
            accumulated = accumulated.saturating_add(estimate_item_tokens(item));
        }
    }
    protect_tool_units(&items, &mut protected);
    let mut protection = Protection::default();
    for (index, segment) in segments.iter().enumerate() {
        if let Some(reason) = protected.get(&index) {
            protection.protected.push(segment.reference.clone());
            protection.reasons.push(reason.clone());
        }
    }
    Ok(protection)
}

fn protect_tool_units(items: &[Option<Value>], protected: &mut HashMap<usize, String>) {
    let mut calls: Vec<(usize, String)> = Vec::new();
    let mut outputs: HashMap<String, usize> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        let Some(item) = item else { continue };
        let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match (kind, call_id) {
            ("function_call" | "custom_tool_call", Some(call_id)) => calls.push((index, call_id)),
            ("function_call_output" | "custom_tool_call_output", Some(call_id)) => {
                outputs.insert(call_id, index);
            }
            _ => {}
        }
    }
    for (index, call_id) in &calls {
        if !outputs.contains_key(call_id) {
            for later in *index..items.len() {
                protected
                    .entry(later)
                    .or_insert_with(|| "incomplete tool call".to_owned());
            }
            break;
        }
    }
    for (index, call_id) in &calls {
        if let Some(output_index) = outputs.get(call_id) {
            if protected.contains_key(index) {
                protected
                    .entry(*output_index)
                    .or_insert_with(|| "tool call unit".to_owned());
            }
            if protected.contains_key(output_index) {
                protected
                    .entry(*index)
                    .or_insert_with(|| "tool call unit".to_owned());
            }
        }
    }
}

fn compress(
    store: &mut Store,
    session: &str,
    revision: i64,
    segments: &[SegmentRow],
    settings: &crate::config::ContextSettings,
    arguments: &Value,
) -> Result<(String, i64)> {
    let summary = arguments
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .context("compress requires a non-empty summary")?;
    let topic = arguments
        .get("topic")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|topic| !topic.is_empty())
        .unwrap_or("context");
    let covered = covered_segments(segments, arguments)?;
    let protection = compute_protection(store, session, segments, settings)?;
    for segment in &covered {
        if protection.protected.contains(&segment.reference) {
            bail!("cannot compress protected segment {}", segment.reference);
        }
    }
    let tier = compression_tier(store, session, &covered)?;
    if tier.as_u8() > settings.max_tier {
        bail!(
            "compression tier {} exceeds the configured max_tier {}",
            tier.as_u8(),
            settings.max_tier
        );
    }
    let mut direct_refs = Vec::new();
    let mut full_refs: Vec<String> = Vec::new();
    let mut covered_tokens = 0u64;
    for segment in &covered {
        direct_refs.push(DirectRef {
            kind: segment.kind,
            reference: segment.reference.clone(),
            position: segment.position,
            end_position: segment.end_position,
        });
        match segment.kind {
            SegmentKind::Message => {
                let raw = store
                    .message_json(session, &segment.reference)?
                    .with_context(|| {
                        format!("archived message {} is missing", segment.reference)
                    })?;
                let item: Value = serde_json::from_str(&raw).context("decode archived message")?;
                covered_tokens = covered_tokens.saturating_add(estimate_item_tokens(&item));
                full_refs.push(segment.reference.clone());
            }
            SegmentKind::Block => {
                let block = store
                    .block(session, &segment.reference)?
                    .with_context(|| format!("context block {} is missing", segment.reference))?;
                let rendered = format!("{} {}", block.topic, block.summary);
                covered_tokens = covered_tokens
                    .saturating_add(estimate_text_tokens(&rendered))
                    .saturating_add(8);
                for reference in block.full_refs {
                    if !full_refs.contains(&reference) {
                        full_refs.push(reference);
                    }
                }
            }
        }
    }
    let replacement_tokens = estimate_text_tokens(topic)
        .saturating_add(estimate_text_tokens(summary))
        .saturating_add(16);
    if replacement_tokens >= covered_tokens {
        bail!(
            "summary does not shrink the context: {} tokens replaced by {}",
            covered_tokens,
            replacement_tokens
        );
    }
    store.apply_compression(
        session,
        revision,
        &covered,
        tier,
        topic,
        summary,
        &direct_refs,
        &full_refs,
        covered_tokens,
        replacement_tokens,
    )
}

fn covered_segments(segments: &[SegmentRow], arguments: &Value) -> Result<Vec<SegmentRow>> {
    let mut indices: Vec<usize> = Vec::new();
    if let Some(ids) = arguments.get("block_ids").and_then(Value::as_array) {
        for id in ids {
            let id = id.as_str().context("block_ids entries must be strings")?;
            let index = segments
                .iter()
                .position(|segment| segment.reference == id)
                .with_context(|| format!("block {id} is not in the current view"))?;
            indices.push(index);
        }
    }
    if let Some(ranges) = arguments.get("ranges").and_then(Value::as_array) {
        for range in ranges {
            let start = range
                .get("start")
                .and_then(Value::as_str)
                .context("range start must be a string")?;
            let end = range
                .get("end")
                .and_then(Value::as_str)
                .context("range end must be a string")?;
            let start_index = segments
                .iter()
                .position(|segment| segment.reference == start)
                .with_context(|| format!("segment {start} is not in the current view"))?;
            let end_index = segments
                .iter()
                .position(|segment| segment.reference == end)
                .with_context(|| format!("segment {end} is not in the current view"))?;
            if start_index > end_index {
                bail!("range {start}..{end} is reversed");
            }
            indices.extend(start_index..=end_index);
        }
    }
    if indices.is_empty() {
        bail!("compress requires ranges or block_ids");
    }
    indices.sort_unstable();
    indices.dedup();
    for pair in indices.windows(2) {
        if pair[0] + 1 != pair[1] {
            bail!("compression range is not contiguous");
        }
    }
    if indices.len() < 2 {
        bail!("compression needs at least two segments");
    }
    Ok(indices
        .iter()
        .map(|index| segments[*index].clone())
        .collect())
}

fn compression_tier(store: &Store, session: &str, covered: &[SegmentRow]) -> Result<Tier> {
    if covered
        .iter()
        .all(|segment| segment.kind == SegmentKind::Message)
    {
        return Ok(Tier::One);
    }
    if covered
        .iter()
        .any(|segment| segment.kind == SegmentKind::Message)
    {
        bail!("cannot mix messages and summaries in one compression");
    }
    let mut tiers = HashSet::new();
    for segment in covered {
        let block = store
            .block(session, &segment.reference)?
            .with_context(|| format!("context block {} is missing", segment.reference))?;
        tiers.insert(block.tier.as_u8());
    }
    if tiers == HashSet::from([1]) {
        return Ok(Tier::Two);
    }
    if tiers.iter().any(|tier| *tier == 1) {
        bail!("cannot mix tier 1 summaries with tier 2 or tier 3 summaries");
    }
    Ok(Tier::Three)
}

fn status(
    store: &Store,
    session: &str,
    revision: i64,
    window_tokens: u64,
    settings: &crate::config::ContextSettings,
) -> Result<ContextStatus> {
    let segments = store.segments(session, revision)?;
    let rendered = render_view(store, session, &segments)?;
    let protection = compute_protection(store, session, &segments, settings)?;
    let mut estimated = 0u64;
    for item in &rendered {
        estimated = estimated.saturating_add(estimate_item_tokens(item));
    }
    estimated = estimated.saturating_add(estimate_text_tokens(GUIDANCE));
    let (tiers, archive_messages, archive_bytes) = store.tier_counts(session, revision)?;
    let compressible =
        compressible_ranges(store, session, &segments, &protection, settings.max_tier)?;
    Ok(ContextStatus {
        session: session.to_owned(),
        revision,
        enabled: settings.enabled,
        window_tokens,
        estimated_view_tokens: estimated,
        growth_threshold_tokens: settings.growth_threshold_tokens,
        pressure: if window_tokens == 0 {
            0.0
        } else {
            estimated as f64 / window_tokens as f64
        },
        tiers,
        archive_messages,
        archive_bytes,
        protected: protection.protected,
        compressible,
    })
}

fn compressible_ranges(
    store: &Store,
    session: &str,
    segments: &[SegmentRow],
    protection: &Protection,
    max_tier: u8,
) -> Result<Vec<CompressibleRange>> {
    let protected: HashSet<&str> = protection.protected.iter().map(String::as_str).collect();
    let mut ranges = Vec::new();
    let mut group: Vec<&SegmentRow> = Vec::new();
    let mut group_tier: Option<u8> = None;
    for segment in segments {
        let tier = if protected.contains(segment.reference.as_str()) {
            None
        } else {
            match segment.kind {
                SegmentKind::Message => Some(1),
                SegmentKind::Block => store
                    .block(session, &segment.reference)?
                    .map(|block| (block.tier.as_u8() + 1).min(3))
                    .filter(|tier| *tier <= max_tier),
            }
        };
        if tier != group_tier || group.is_empty() && tier.is_none() {
            if group.len() >= 2 {
                ranges.push(range_of(store, session, &group, group_tier.unwrap_or(1))?);
            }
            group.clear();
            group_tier = tier;
        }
        if tier.is_some() {
            group.push(segment);
        }
    }
    if group.len() >= 2 {
        ranges.push(range_of(store, session, &group, group_tier.unwrap_or(1))?);
    }
    Ok(ranges)
}

/// Only content-bearing fields count toward the kernel character gate.
fn message_chars(value: &Value) -> usize {
    match value {
        Value::String(text) => text.encode_utf16().count(),
        Value::Array(values) => values.iter().map(message_chars).sum(),
        Value::Object(object) => ["text", "content", "output", "arguments", "input"]
            .iter()
            .filter_map(|key| object.get(*key))
            .map(message_chars)
            .sum(),
        _ => 0,
    }
}

fn range_of(
    store: &Store,
    session: &str,
    group: &[&SegmentRow],
    tier: u8,
) -> Result<CompressibleRange> {
    let mut tokens = 0u64;
    for segment in group {
        match segment.kind {
            SegmentKind::Message => {
                let raw = store
                    .message_json(session, &segment.reference)?
                    .with_context(|| {
                        format!("archived message {} is missing", segment.reference)
                    })?;
                let item: Value = serde_json::from_str(&raw).context("decode archived message")?;
                tokens = tokens.saturating_add(estimate_item_tokens(&item));
            }
            SegmentKind::Block => {
                let block = store
                    .block(session, &segment.reference)?
                    .with_context(|| format!("context block {} is missing", segment.reference))?;
                tokens = tokens
                    .saturating_add(estimate_text_tokens(&format!(
                        "{} {}",
                        block.topic, block.summary
                    )))
                    .saturating_add(8);
            }
        }
    }
    Ok(CompressibleRange {
        start: group
            .first()
            .map(|segment| segment.reference.clone())
            .unwrap_or_default(),
        end: group
            .last()
            .map(|segment| segment.reference.clone())
            .unwrap_or_default(),
        tier: format!("t{tier}"),
        estimated_tokens: tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ContextSettings;
    use crate::context::types::{
        SESSION_SEPARATOR, TOOL_COMPRESS, TOOL_DECOMPRESS, TOOL_SEARCH, TOOL_STATUS,
    };

    fn settings() -> ContextSettings {
        ContextSettings {
            enabled: true,
            protected_messages: 1,
            protected_tokens: 1,
            ..ContextSettings::default()
        }
    }

    fn engine() -> ContextEngine {
        ContextEngine::open_in_memory(settings()).expect("in-memory context engine")
    }

    fn session() -> String {
        format!("codex{SESSION_SEPARATOR}thread-1")
    }

    fn message(role: &str, text: &str) -> Value {
        json!({
            "type": "message",
            "role": role,
            "content": [{"type": "input_text", "text": text}],
        })
    }

    fn long_body(label: &str, size: usize) -> String {
        format!("{label} {}", "word ".repeat(size / 5))
    }

    fn status_of(engine: &ContextEngine, session: &str, revision: i64) -> ContextStatus {
        let execution = engine
            .execute_tool(session, revision, TOOL_STATUS, &json!({}), 10_000)
            .expect("context status");
        serde_json::from_str(&execution.content).expect("decode context status")
    }

    fn compress_pair(
        engine: &ContextEngine,
        session: &str,
        revision: i64,
        range: &CompressibleRange,
    ) -> ToolExecution {
        engine
            .execute_tool(
                session,
                revision,
                TOOL_COMPRESS,
                &json!({
                    "ranges": [{"start": range.start, "end": range.end}],
                    "topic": "work",
                    "summary": "The model produced two long answers about the task.",
                }),
                10_000,
            )
            .expect("compress messages")
    }

    fn four_messages() -> Vec<Value> {
        vec![
            message("user", "please start the task"),
            message("assistant", &long_body("alpha", 900)),
            message("assistant", &long_body("beta", 900)),
            message("user", "continue"),
        ]
    }

    #[test]
    fn compresses_a_message_pair_into_a_tier_one_block() {
        let engine = engine();
        let session = session();
        let items = four_messages();
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        assert_eq!(view.items.len(), 4);
        assert!(view.protection.protected.len() >= 2);

        let status = status_of(&engine, &session, view.revision);
        let range = status
            .compressible
            .iter()
            .find(|range| range.tier == "t1")
            .expect("a tier 1 range");
        let execution = compress_pair(&engine, &session, view.revision, range);
        assert!(execution.changed);

        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        assert_eq!(view.revision, execution.revision);
        assert_eq!(view.items.len(), 3);
        let summary = view.items[1]["content"][0]["text"]
            .as_str()
            .expect("summary text");
        assert!(summary.contains("<context_summary"));
        assert!(summary.contains("tier=\"1\""));

        let export = engine.export(&session).expect("export");
        assert_eq!(export["history"].as_array().expect("history").len(), 4);
        assert_eq!(export["blocks"].as_array().expect("blocks").len(), 1);
    }

    fn compress_refs(
        engine: &ContextEngine,
        session: &str,
        revision: i64,
        start: &str,
        end: &str,
    ) -> ToolExecution {
        engine
            .execute_tool(
                session,
                revision,
                TOOL_COMPRESS,
                &json!({
                    "ranges": [{"start": start, "end": end}],
                    "topic": "work",
                    "summary": "The model answered the same task twice.",
                }),
                10_000,
            )
            .expect("compress range")
    }

    fn segment_refs(engine: &ContextEngine, session: &str) -> Vec<String> {
        engine.export(session).expect("export")["segments"]
            .as_array()
            .expect("segments")
            .iter()
            .map(|segment| segment["reference"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn nudge_persists_and_resets_after_compression() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("context.db");
        let engine = ContextEngine::open(settings(), &path).expect("engine");
        let session = session();
        let mut items = four_messages();
        items[1] = message("assistant", &long_body("first", 120_000));
        items[2] = message("assistant", &long_body("second", 120_000));
        // Enough pending mass, but a fresh low-pressure view seeds a baseline.
        let view = engine
            .begin_turn(&session, &items, 1_000_000)
            .expect("view");
        let baseline = view.estimated_tokens;
        assert!(
            engine
                .pressure_note(&session, &view, true)
                .expect("note")
                .contains("keep working")
        );
        drop(engine);
        let engine = ContextEngine::open(settings(), &path).expect("reopen");
        items.push(message("assistant", &long_body("growth", 90_000)));
        items.push(message("user", "continue"));
        let view = engine
            .begin_turn(&session, &items, 1_000_000)
            .expect("grown view");
        assert!(view.estimated_tokens >= baseline + 22_500);
        assert!(
            engine
                .pressure_note(&session, &view, true)
                .expect("note")
                .contains("GROWTH T1")
        );
        assert!(
            engine
                .pressure_note(&session, &view, true)
                .expect("note")
                .contains("keep working")
        );
        let refs = segment_refs(&engine, &session);
        compress_refs(&engine, &session, view.revision, &refs[1], &refs[2]);
        // A successful commit clears the pending stamp. Remaining backlog can
        // trigger first-sight mass immediately, without waiting for new growth.
        items.push(message("assistant", &long_body("backlog", 240_000)));
        items.push(message("user", "continue again"));
        let mut view = engine
            .begin_turn(&session, &items, 1_000_000)
            .expect("remaining view");
        view.estimated_tokens = 450_000;
        assert!(
            engine
                .pressure_note(&session, &view, true)
                .expect("note")
                .contains("GROWTH T1")
        );
    }

    #[test]
    fn tiny_ranges_do_not_trigger_a_pressure_nudge() {
        let engine = engine();
        let session = session();
        let mut items = four_messages();
        items[1] = message("assistant", &"x".repeat(2_000));
        items[2] = message("assistant", &"x".repeat(2_000));
        let mut view = engine.begin_turn(&session, &items, 100_000).expect("view");
        view.estimated_tokens = 95_000;
        assert!(
            engine
                .pressure_note(&session, &view, true)
                .expect("note")
                .contains("keep working")
        );
        assert_eq!(
            message_chars(&json!({"content": [{"text": "\u{1f600}"}]})),
            2
        );
    }

    #[test]
    fn recommends_raw_messages_separately() {
        let engine = engine();
        let session = session();
        let mut items = four_messages();
        items.insert(1, message("assistant", &long_body("gamma", 900)));
        items.insert(2, message("assistant", &long_body("delta", 900)));
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let refs = segment_refs(&engine, &session);
        let first = compress_refs(&engine, &session, view.revision, &refs[1], &refs[2]);
        let status = status_of(&engine, &session, first.revision);
        assert_eq!(status.compressible.len(), 1);
        let range = &status.compressible[0];
        assert_eq!(range.start, refs[3]);
        assert_eq!(range.end, refs[4]);
        assert_eq!(range.tier, "t1");
        let second = compress_pair(&engine, &session, first.revision, range);
        assert_eq!(
            status_of(&engine, &session, second.revision)
                .tiers
                .t1_blocks,
            2
        );
    }

    #[test]
    fn escalates_two_tier_one_blocks_into_a_tier_two_block() {
        let engine = engine();
        let session = session();
        let mut items = four_messages();
        items.insert(1, message("assistant", &long_body("gamma", 900)));
        items.insert(2, message("assistant", &long_body("delta", 900)));
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let refs = segment_refs(&engine, &session);
        let first = compress_refs(&engine, &session, view.revision, &refs[1], &refs[2]);

        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        assert_eq!(view.items.len(), 5);
        let refs = segment_refs(&engine, &session);
        let second = compress_refs(&engine, &session, view.revision, &refs[2], &refs[3]);

        let status = status_of(&engine, &session, second.revision);
        assert_eq!(status.tiers.t1_blocks, 2);
        assert_eq!(status.compressible.len(), 1);
        assert_eq!(status.compressible[0].tier, "t2");
        let refs = segment_refs(&engine, &session);
        let execution = engine
            .execute_tool(
                &session,
                second.revision,
                TOOL_COMPRESS,
                &json!({
                    "block_ids": [refs[1], refs[2]],
                    "topic": "work",
                    "summary": "Both answers covered the same task.",
                }),
                10_000,
            )
            .expect("compress blocks");
        let status = status_of(&engine, &session, execution.revision);
        assert_eq!(status.tiers.t2_blocks, 1);
        assert_eq!(status.tiers.t1_blocks, 0);
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        assert_eq!(view.items.len(), 3);
        assert_eq!(view.revision, execution.revision);
        assert_eq!(first.revision + 1, second.revision);
    }

    #[test]
    fn refuses_a_tier_above_the_configured_max_tier() {
        let engine = ContextEngine::open_in_memory(ContextSettings {
            max_tier: 1,
            ..settings()
        })
        .expect("in-memory context engine");
        let session = session();
        let mut items = four_messages();
        items.insert(1, message("assistant", &long_body("gamma", 900)));
        items.insert(2, message("assistant", &long_body("delta", 900)));
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let refs = segment_refs(&engine, &session);
        let first = compress_refs(&engine, &session, view.revision, &refs[1], &refs[2]);

        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let refs = segment_refs(&engine, &session);
        let second = compress_refs(&engine, &session, view.revision, &refs[2], &refs[3]);
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        assert_eq!(view.revision, second.revision);
        assert!(
            status_of(&engine, &session, second.revision)
                .compressible
                .is_empty()
        );
        let refs = segment_refs(&engine, &session);
        let error = engine
            .execute_tool(
                &session,
                second.revision,
                TOOL_COMPRESS,
                &json!({
                    "block_ids": [refs[1], refs[2]],
                    "topic": "work",
                    "summary": "Both answers covered the same task.",
                }),
                10_000,
            )
            .expect_err("tier 2 must be refused");
        assert!(
            format!("{error}").contains("exceeds the configured max_tier 1"),
            "unexpected error: {error}"
        );
        assert_eq!(first.revision + 1, second.revision);
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        assert_eq!(view.revision, second.revision);
    }

    #[test]
    fn rejects_protected_ranges() {
        let engine = engine();
        let session = session();
        let items = four_messages();
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let export = engine.export(&session).expect("export");
        let segments = export["segments"].as_array().expect("segments");
        let start = segments[0]["reference"].as_str().expect("reference");
        let end = segments[1]["reference"].as_str().expect("reference");
        let error = engine
            .execute_tool(
                &session,
                view.revision,
                TOOL_COMPRESS,
                &json!({"ranges": [{"start": start, "end": end}], "summary": "short"}),
                10_000,
            )
            .expect_err("protected range must fail");
        assert!(format!("{error}").contains("cannot compress protected segment"));
    }

    #[test]
    fn rejects_illegal_ranges() {
        let engine = engine();
        let session = session();
        let items = four_messages();
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let export = engine.export(&session).expect("export");
        let refs: Vec<String> = export["segments"]
            .as_array()
            .expect("segments")
            .iter()
            .map(|segment| segment["reference"].as_str().unwrap_or_default().to_owned())
            .collect();

        let cases = [
            (
                json!({"ranges": [{"start": refs[1], "end": refs[1]}], "summary": "short"}),
                "at least two segments",
            ),
            (
                json!({"ranges": [{"start": refs[2], "end": refs[1]}], "summary": "short"}),
                "is reversed",
            ),
            (
                json!({"block_ids": ["m999999"], "summary": "short"}),
                "not in the current view",
            ),
            (
                json!({"ranges": [{"start": refs[1], "end": refs[1]}, {"start": refs[3], "end": refs[3]}], "summary": "short"}),
                "not contiguous",
            ),
            (json!({"summary": "short"}), "requires ranges or block_ids"),
            (
                json!({"ranges": [{"start": refs[1], "end": refs[2]}]}),
                "non-empty summary",
            ),
        ];
        for (arguments, expected) in cases {
            let error = engine
                .execute_tool(&session, view.revision, TOOL_COMPRESS, &arguments, 10_000)
                .expect_err("illegal compression");
            assert!(
                format!("{error}").contains(expected),
                "unexpected error for {arguments}: {error}"
            );
        }
    }

    #[test]
    fn rejects_non_shrinking_summaries() {
        let engine = engine();
        let session = session();
        let items = four_messages();
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let status = status_of(&engine, &session, view.revision);
        let range = status.compressible.first().expect("compressible range");
        let error = engine
            .execute_tool(
                &session,
                view.revision,
                TOOL_COMPRESS,
                &json!({
                    "ranges": [{"start": range.start, "end": range.end}],
                    "topic": "work",
                    "summary": long_body("verbose", 3_000),
                }),
                10_000,
            )
            .expect_err("non-shrinking summary");
        assert!(format!("{error}").contains("does not shrink the context"));
    }

    #[test]
    fn rejects_stale_revisions() {
        let engine = engine();
        let session = session();
        let items = four_messages();
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let error = engine
            .execute_tool(&session, view.revision - 1, TOOL_STATUS, &json!({}), 10_000)
            .expect_err("stale revision");
        assert!(format!("{error}").contains("context revision changed"));
    }

    #[test]
    fn serializes_turns_of_one_session_with_a_lease() {
        let engine = std::sync::Arc::new(engine());
        let session = session();
        let first = engine.acquire_lease(&session).expect("first lease");
        let error = match engine.acquire_lease(&session) {
            Ok(_) => panic!("a second turn must not take the same session"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("busy"), "{error}");
        let other = format!("codex{SESSION_SEPARATOR}thread-2");
        let second = engine.acquire_lease(&other).expect("other session");
        drop(second);
        drop(first);
        engine
            .acquire_lease(&session)
            .expect("the session is free after the turn ends");
    }

    #[test]
    fn restores_a_block_into_raw_messages() {
        let engine = engine();
        let session = session();
        let items = four_messages();
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let status = status_of(&engine, &session, view.revision);
        let range = status
            .compressible
            .first()
            .expect("compressible range")
            .clone();
        let compressed = compress_pair(&engine, &session, view.revision, &range);
        let block_id = engine.export(&session).expect("export")["blocks"][0]["id"]
            .as_str()
            .expect("block id")
            .to_owned();

        let preview = engine
            .execute_tool(
                &session,
                compressed.revision,
                TOOL_DECOMPRESS,
                &json!({"block_id": block_id, "apply": false}),
                10_000,
            )
            .expect("decompress preview");
        assert!(!preview.changed);
        assert!(preview.content.contains(&block_id));

        let restored = engine
            .execute_tool(
                &session,
                compressed.revision,
                TOOL_DECOMPRESS,
                &json!({"block_id": block_id, "apply": true}),
                10_000,
            )
            .expect("restore block");
        assert!(restored.changed);
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        assert_eq!(view.revision, restored.revision);
        assert_eq!(view.items.len(), 4);
        assert!(
            !view
                .items
                .iter()
                .any(|item| item.to_string().contains("<context_summary"))
        );
    }

    #[test]
    fn search_finds_archived_messages() {
        let engine = engine();
        let session = session();
        let items = vec![
            message("user", "please start the task"),
            message(
                "assistant",
                &format!("zebracrossing {}", long_body("alpha", 900)),
            ),
            message("assistant", &long_body("beta", 900)),
            message("user", "continue"),
        ];
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let status = status_of(&engine, &session, view.revision);
        let range = status
            .compressible
            .first()
            .expect("compressible range")
            .clone();
        compress_pair(&engine, &session, view.revision, &range);

        let execution = engine
            .execute_tool(
                &session,
                view.revision + 1,
                TOOL_SEARCH,
                &json!({"query": "zebracrossing"}),
                10_000,
            )
            .expect("search");
        assert!(execution.content.contains("zebracrossing"));
        assert!(!execution.changed);
    }

    #[test]
    fn rewrites_the_view_when_the_client_changes_history() {
        let engine = engine();
        let session = session();
        let items = four_messages();
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let status = status_of(&engine, &session, view.revision);
        let range = status
            .compressible
            .first()
            .expect("compressible range")
            .clone();
        let compressed = compress_pair(&engine, &session, view.revision, &range);

        let mut forked_items = items.clone();
        forked_items[1] = message("assistant", "edited answer");
        let view = engine
            .begin_turn(&session, &forked_items, 10_000)
            .expect("view");
        assert!(view.revision > compressed.revision);
        assert_eq!(view.items.len(), 4);
        assert!(
            !view
                .items
                .iter()
                .any(|item| item.to_string().contains("<context_summary"))
        );
        let export = engine.export(&session).expect("export");
        assert_eq!(
            export["history"][1]["item"]["content"][0]["text"],
            "edited answer"
        );
        let sessions = engine.sessions().expect("sessions");
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn keeps_the_compressed_view_when_the_client_resends_its_history() {
        let engine = engine();
        let session = session();
        let items = four_messages();
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let status = status_of(&engine, &session, view.revision);
        let range = status
            .compressible
            .first()
            .expect("compressible range")
            .clone();
        let compressed = compress_pair(&engine, &session, view.revision, &range);

        // The next turn replays the history the client knows, plus the answer
        // it saw and the new user message; the gateway never stores internal
        // turns, so the compressed block must survive this replay.
        let mut next = items.clone();
        next.push(message("assistant", "the answer the client saw"));
        next.push(message("user", "next question"));
        let view = engine.begin_turn(&session, &next, 10_000).expect("view");
        assert_eq!(view.revision, compressed.revision);
        assert_eq!(view.items.len(), 5);
        let summary = view.items[1]["content"][0]["text"]
            .as_str()
            .expect("summary text");
        assert!(summary.contains("<context_summary"), "{summary}");
    }

    #[test]
    fn expands_a_chained_request_from_the_archive() {
        let engine = engine();
        let session = session();
        let items = four_messages();
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let answer = message("assistant", "done with alpha and beta");
        engine
            .record_response(
                &session,
                "resp_1",
                view.revision,
                std::slice::from_ref(&answer),
            )
            .expect("record response");

        let next = message("user", "next question");
        let expanded = engine
            .expand_chained_input(&session, "resp_1", std::slice::from_ref(&next))
            .expect("expand");
        assert_eq!(expanded.len(), items.len() + 2);
        assert_eq!(fingerprint(&expanded[0]), fingerprint(&items[0]));
        assert_eq!(fingerprint(&expanded[items.len()]), fingerprint(&answer));
        assert_eq!(
            expanded[items.len() + 1]["content"][0]["text"],
            "next question"
        );

        // A client that resends the whole history must not get it duplicated.
        let mut resent = expanded.clone();
        resent.push(message("user", "another question"));
        let unchanged = engine
            .expand_chained_input(&session, "resp_1", &resent)
            .expect("expand resent history");
        assert_eq!(unchanged.len(), resent.len());
        assert_eq!(fingerprint(&unchanged[0]), fingerprint(&resent[0]));
    }

    #[test]
    fn rejects_an_unknown_response_id_without_history() {
        let engine = engine();
        let session = session();
        let error = engine
            .expand_chained_input(&session, "resp_missing", &[])
            .expect_err("unknown response id");
        assert!(error.to_string().contains("unknown response id"), "{error}");

        let incoming = vec![message("user", "hello")];
        let items = engine
            .expand_chained_input(&session, "resp_missing", &incoming)
            .expect("history without a mapping");
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn rejects_a_response_id_from_another_session() {
        let engine = engine();
        let session = session();
        let other = format!("codex{SESSION_SEPARATOR}thread-2");
        let view = engine
            .begin_turn(&other, &four_messages(), 10_000)
            .expect("view");
        engine
            .record_response(&other, "resp_other", view.revision, &[])
            .expect("record response");

        let error = engine
            .expand_chained_input(&session, "resp_other", &[])
            .expect_err("cross session response id");
        assert!(
            error.to_string().contains("belongs to context session"),
            "{error}"
        );
    }

    fn checkpoint_secret() -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7_u8; 32])
    }

    fn compress_first_pair(engine: &ContextEngine, session: &str) -> i64 {
        let items = four_messages();
        let view = engine.begin_turn(session, &items, 10_000).expect("view");
        let status = status_of(engine, session, view.revision);
        let range = status
            .compressible
            .iter()
            .find(|range| range.tier == "t1")
            .expect("a tier 1 range");
        compress_pair(engine, session, view.revision, range).revision
    }

    #[test]
    fn expands_a_checkpoint_into_the_archived_view() {
        let engine = engine();
        let session = session();
        let revision = compress_first_pair(&engine, &session);
        let token = checkpoint::encode("model-a", &session, revision, &checkpoint_secret())
            .expect("encode checkpoint");
        let items = vec![
            json!({"type": "compaction", "encrypted_content": token}),
            message("user", "next question"),
        ];
        let expanded = engine
            .expand_checkpoint(&session, &items, &checkpoint_secret())
            .expect("expand")
            .expect("a checkpoint");
        // The archive keeps every original message, so a checkpoint expands to
        // the full history; the next turn then renders the compressed view
        // because the fingerprints still match.
        assert_eq!(expanded.len(), 5);
        assert_eq!(
            expanded[0]["content"][0]["text"].as_str(),
            Some("please start the task")
        );
        assert_eq!(expanded[3]["content"][0]["text"].as_str(), Some("continue"));
        assert_eq!(
            expanded[4]["content"][0]["text"].as_str(),
            Some("next question")
        );
        let view = engine
            .begin_turn(&session, &expanded, 10_000)
            .expect("view after checkpoint");
        assert_eq!(view.items.len(), 4);
        assert!(
            view.items[1]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("<context_summary"))
        );

        let plain = vec![message("user", "no checkpoint here")];
        assert!(
            engine
                .expand_checkpoint(&session, &plain, &checkpoint_secret())
                .expect("plain input")
                .is_none()
        );
    }

    #[test]
    fn rejects_a_checkpoint_from_another_session() {
        let engine = engine();
        let session = session();
        let other = format!("codex{SESSION_SEPARATOR}thread-2");
        let view = engine
            .begin_turn(&other, &four_messages(), 10_000)
            .expect("view");
        let token = checkpoint::encode("model-a", &other, view.revision, &checkpoint_secret())
            .expect("encode checkpoint");
        let items = vec![json!({"type": "compaction", "encrypted_content": token})];
        let error = engine
            .expand_checkpoint(&session, &items, &checkpoint_secret())
            .expect_err("cross session checkpoint");
        assert!(
            error.to_string().contains("belongs to context session"),
            "{error}"
        );
    }

    #[test]
    fn rejects_a_checkpoint_newer_than_the_archive() {
        let engine = engine();
        let session = session();
        let view = engine
            .begin_turn(&session, &four_messages(), 10_000)
            .expect("view");
        let token =
            checkpoint::encode("model-a", &session, view.revision + 1, &checkpoint_secret())
                .expect("encode checkpoint");
        let items = vec![json!({"type": "compaction", "encrypted_content": token})];
        let error = engine
            .expand_checkpoint(&session, &items, &checkpoint_secret())
            .expect_err("newer revision");
        assert!(
            error.to_string().contains("is newer than the archived"),
            "{error}"
        );
    }

    #[test]
    fn reports_compression_and_cache_stats() {
        let engine = engine();
        let session = session();
        let items = four_messages();
        let view = engine.begin_turn(&session, &items, 10_000).expect("view");
        let status = status_of(&engine, &session, view.revision);
        let range = status
            .compressible
            .iter()
            .find(|range| range.tier == "t1")
            .expect("a tier 1 range");
        let execution = compress_pair(&engine, &session, view.revision, range);
        engine
            .record_turn(
                &session,
                TurnSample {
                    at_ms: 1,
                    input_tokens: 1_000,
                    cached_tokens: 600,
                    output_tokens: 50,
                },
            )
            .expect("first turn");
        engine
            .record_turn(
                &session,
                TurnSample {
                    at_ms: 2,
                    input_tokens: 1_000,
                    cached_tokens: 700,
                    output_tokens: 80,
                },
            )
            .expect("second turn");

        let stats = engine.stats(10_000, true).expect("context stats");
        assert!(stats.enabled);
        assert_eq!(stats.totals.sessions, 1);
        assert_eq!(stats.totals.messages, 4);
        assert_eq!(stats.totals.compressions, 1);
        assert!(stats.totals.saved_tokens > 0);
        assert_eq!(stats.totals.cache.turns, 2);
        assert_eq!(stats.totals.cache.input_tokens, 2_000);
        assert_eq!(stats.totals.cache.cached_tokens, 1_300);
        assert_eq!(stats.totals.cache.output_tokens, 130);
        assert!((stats.totals.cache.hit_percent - 65.0).abs() < 1e-9);

        let row = stats.sessions.first().expect("one session");
        assert_eq!(row.session, session);
        assert_eq!(row.display, "codex / thread-1");
        assert_eq!(row.revision, execution.revision);
        assert_eq!(row.compressions, 1);
        assert!(row.view_tokens > 0);
        assert_eq!(row.tiers.t1_blocks, 1);
        assert_eq!(row.cache.hit_percent, 65.0);
    }

    #[test]
    fn upgrades_a_database_written_before_token_columns() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("context.db");
        drop(ContextEngine::open(settings(), &path).expect("first open"));
        {
            let connection = rusqlite::Connection::open(&path).expect("open database");
            connection
                .execute_batch(
                    "DROP TABLE blocks;
                     CREATE TABLE blocks (
                        session_key TEXT NOT NULL,
                        block_id TEXT NOT NULL,
                        tier INTEGER NOT NULL,
                        topic TEXT NOT NULL,
                        summary TEXT NOT NULL,
                        direct_refs TEXT NOT NULL,
                        full_refs TEXT NOT NULL,
                        start_position INTEGER NOT NULL,
                        end_position INTEGER NOT NULL,
                        superseded_by TEXT,
                        created_at INTEGER NOT NULL,
                        PRIMARY KEY (session_key, block_id)
                     );",
                )
                .expect("recreate the old blocks table");
        }

        let engine = ContextEngine::open(settings(), &path).expect("upgrade open");
        let session = session();
        let view = engine
            .begin_turn(&session, &four_messages(), 10_000)
            .expect("view");
        let status = status_of(&engine, &session, view.revision);
        let range = status
            .compressible
            .iter()
            .find(|range| range.tier == "t1")
            .expect("a tier 1 range");
        compress_pair(&engine, &session, view.revision, range);

        let stats = engine.stats(10_000, true).expect("context stats");
        assert_eq!(stats.totals.compressions, 1);
        assert!(
            stats.totals.saved_tokens > 0,
            "token columns must be upgraded"
        );
    }

    #[test]
    fn reports_zero_stats_without_sessions() {
        let engine = engine();
        let stats = engine.stats(10_000, false).expect("context stats");
        assert!(!stats.enabled);
        assert!(stats.sessions.is_empty());
        assert_eq!(stats.totals.sessions, 0);
        assert_eq!(stats.totals.saved_tokens, 0);
        assert_eq!(stats.totals.cache.hit_percent, 0.0);
    }

    #[test]
    fn reports_pressure_and_archive_size() {
        let engine = engine();
        let session = session();
        let items = four_messages();
        let view = engine.begin_turn(&session, &items, 2_000).expect("view");
        let status = status_of(&engine, &session, view.revision);
        assert_eq!(status.window_tokens, 10_000);
        assert!((status.pressure - status.estimated_view_tokens as f64 / 10_000.0).abs() < 1e-9);
        assert_eq!(status.archive_messages, 4);
        assert!(status.archive_bytes > 0);
        assert!(view.pressure() > 0.0);
    }
}
