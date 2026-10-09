//! Time-bucketed and per-client views of provider usage for the dashboard.

use std::sync::PoisonError;

use rusqlite::{Connection, params};
use serde::Serialize;

use super::{
    TokenUsageAggregator, current_unix_day, current_unix_millis, ensure_request_usage_table,
};

const HOUR_MILLIS: u64 = 3_600_000;
const DAY_MILLIS: u64 = 86_400_000;
const HOURLY_WINDOW_HOURS: u64 = 24;
const HEATMAP_DAYS: u64 = 365;

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct UsageActivity {
    /// Last 24 hours from the request ledger, one row per (hour, model).
    pub(crate) hourly: Vec<HourlyModelUsage>,
    /// Last year of UTC days from the daily aggregate, for the heatmap.
    pub(crate) daily: Vec<DailyUsage>,
    /// Per-client totals over the requested range, capped by ledger retention.
    pub(crate) clients: Vec<ClientUsage>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HourlyModelUsage {
    pub(crate) hour_start_ms: u64,
    pub(crate) model_id: String,
    pub(crate) request_count: u64,
    pub(crate) total_tokens: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DailyUsage {
    pub(crate) day: u64,
    pub(crate) request_count: u64,
    pub(crate) total_tokens: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ClientUsage {
    pub(crate) client_id: String,
    pub(crate) request_count: u64,
    pub(crate) input_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) average_ttft_ms: Option<f64>,
    pub(crate) output_tps: Option<f64>,
}

impl TokenUsageAggregator {
    pub(crate) fn activity_snapshot(
        &self,
        provider_id: &str,
        days: Option<u64>,
    ) -> anyhow::Result<UsageActivity> {
        let mut activity = UsageActivity {
            daily: self.daily_activity(provider_id)?,
            ..UsageActivity::default()
        };
        let Some(path) = &self.persist_path else {
            return Ok(activity);
        };
        let now_ms = current_unix_millis()?;
        let connection = Connection::open(path)?;
        ensure_request_usage_table(&connection)?;
        let hour_cutoff = (now_ms / HOUR_MILLIS + 1 - HOURLY_WINDOW_HOURS) * HOUR_MILLIS;
        activity.hourly = hourly_activity(&connection, provider_id, hour_cutoff)?;
        let client_cutoff = days.map_or(0, |days| now_ms.saturating_sub(days * DAY_MILLIS));
        activity.clients = client_activity(&connection, provider_id, client_cutoff)?;
        Ok(activity)
    }

    fn daily_activity(&self, provider_id: &str) -> anyhow::Result<Vec<DailyUsage>> {
        let first_day = current_unix_day()?.saturating_sub(HEATMAP_DAYS - 1);
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut days = std::collections::BTreeMap::<u64, DailyUsage>::new();
        for ((day, entry_provider, _), usage) in &state.daily_entries {
            if *day < first_day || entry_provider != provider_id {
                continue;
            }
            let bucket = days.entry(*day).or_insert(DailyUsage {
                day: *day,
                request_count: 0,
                total_tokens: 0,
            });
            bucket.request_count = bucket.request_count.saturating_add(usage.request_count);
            bucket.total_tokens = bucket.total_tokens.saturating_add(
                usage
                    .input_tokens
                    .saturating_add(usage.cache_read_tokens)
                    .saturating_add(usage.cache_creation_tokens)
                    .saturating_add(usage.output_tokens),
            );
        }
        Ok(days.into_values().collect())
    }
}

const TOTAL_TOKENS_SQL: &str =
    "SUM(input_tokens + cache_read_tokens + cache_creation_tokens + output_tokens)";

fn hourly_activity(
    connection: &Connection,
    provider_id: &str,
    cutoff_ms: u64,
) -> anyhow::Result<Vec<HourlyModelUsage>> {
    let mut statement = connection.prepare(&format!(
        "SELECT (recorded_at_ms / {HOUR_MILLIS}) * {HOUR_MILLIS} AS hour, model_id,
                COUNT(*), {TOTAL_TOKENS_SQL}
         FROM request_usage
         WHERE provider_id = ?1 AND recorded_at_ms >= ?2
         GROUP BY hour, model_id
         ORDER BY hour, model_id"
    ))?;
    let rows = statement.query_map(params![provider_id, cutoff_ms], |row| {
        Ok(HourlyModelUsage {
            hour_start_ms: row.get(0)?,
            model_id: row.get(1)?,
            request_count: row.get(2)?,
            total_tokens: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn client_activity(
    connection: &Connection,
    provider_id: &str,
    cutoff_ms: u64,
) -> anyhow::Result<Vec<ClientUsage>> {
    let mut statement = connection.prepare(
        "SELECT client_id, COUNT(*), SUM(input_tokens), SUM(cache_read_tokens),
                SUM(cache_creation_tokens), SUM(output_tokens),
                AVG(ttft_micros) / 1000.0,
                SUM(CASE WHEN generation_micros > 0 THEN output_tokens END) * 1000000.0
                    / SUM(CASE WHEN generation_micros > 0 THEN generation_micros END)
         FROM request_usage
         WHERE provider_id = ?1 AND recorded_at_ms >= ?2
         GROUP BY client_id
         ORDER BY SUM(input_tokens + cache_read_tokens + cache_creation_tokens + output_tokens)
             DESC",
    )?;
    let rows = statement.query_map(params![provider_id, cutoff_ms], |row| {
        Ok(ClientUsage {
            client_id: row.get(0)?,
            request_count: row.get(1)?,
            input_tokens: row.get(2)?,
            cache_read_tokens: row.get(3)?,
            cache_creation_tokens: row.get(4)?,
            output_tokens: row.get(5)?,
            average_ttft_ms: row.get(6)?,
            output_tps: row.get(7)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::tempdir;

    use super::super::{
        ProviderRequestUsage, TokenUsageState, UpstreamCacheUsage, persist_usage_delta,
    };
    use super::*;

    #[test]
    fn activity_buckets_hours_and_groups_clients() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("usage.sqlite3");
        let usage = UpstreamCacheUsage {
            input_tokens: Some(10),
            cache_read_tokens: Some(90),
            output_tokens: Some(20),
            ttft_micros: Some(125_000),
            generation_micros: Some(400_000),
            ..Default::default()
        };
        let now_ms = current_unix_millis().unwrap();
        for (age_ms, client_id, provider_id) in [
            (0, "codex", "provider"),
            (2 * HOUR_MILLIS, "codex", "provider"),
            (30 * HOUR_MILLIS, "claude", "provider"),
            (0, "dsh", "other"),
        ] {
            let mut request = ProviderRequestUsage::from_usage(provider_id, "model", &usage);
            request.recorded_at_ms = now_ms - age_ms;
            request.client_id = client_id.to_owned();
            persist_usage_delta(&path, &request).unwrap();
        }
        let aggregator = TokenUsageAggregator {
            state: Mutex::new(TokenUsageState::default()),
            persist_path: Some(path),
        };

        let activity = aggregator.activity_snapshot("provider", None).unwrap();
        // The 30-hour-old row falls outside the 24-hour window.
        assert_eq!(activity.hourly.len(), 2);
        assert!(activity.hourly.iter().all(|bucket| bucket.total_tokens == 120));
        assert_eq!(activity.clients.len(), 2);
        assert_eq!(activity.clients[0].client_id, "codex");
        assert_eq!(activity.clients[0].request_count, 2);
        assert_eq!(activity.clients[0].average_ttft_ms, Some(125.0));
        assert_eq!(activity.clients[0].output_tps, Some(50.0));

        let last_day = aggregator.activity_snapshot("provider", Some(1)).unwrap();
        assert_eq!(last_day.clients.len(), 1);
    }
}
