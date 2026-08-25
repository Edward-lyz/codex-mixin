use std::fs::{self, File, OpenOptions};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::cli::atomic_file::write_atomic_if_changed;

const QUEUE_DIRECTORY: &str = "ducx-report-queue";
const DELIVERED_DIRECTORY: &str = "ducx-report-delivered";
const QUEUE_VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct QueuedReport {
    pub(super) version: u8,
    pub(super) id: String,
    pub(super) created_at_ms: u64,
    pub(super) event: String,
    pub(super) provider_id: String,
    pub(super) hook_body: Value,
}

#[derive(Debug)]
pub(super) struct EnqueueResult {
    pub(super) id: String,
    pub(super) already_delivered: bool,
}

pub(super) fn enqueue_at(
    state_directory: &Path,
    event: &str,
    provider_id: &str,
    event_instance: &str,
    hook_body: &[u8],
) -> anyhow::Result<EnqueueResult> {
    let hook_body = serde_json::from_slice::<Value>(hook_body)
        .context("parse Codex report hook body before queueing")?;
    let canonical_body = serde_json::to_vec(&hook_body)?;
    let mut identity = Vec::with_capacity(
        event.len() + provider_id.len() + event_instance.len() + canonical_body.len() + 3,
    );
    identity.extend_from_slice(event.as_bytes());
    identity.push(0);
    identity.extend_from_slice(provider_id.as_bytes());
    identity.push(0);
    identity.extend_from_slice(event_instance.as_bytes());
    identity.push(0);
    identity.extend_from_slice(&canonical_body);
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, &identity)
        .simple()
        .to_string();
    let delivered_path = state_directory
        .join(DELIVERED_DIRECTORY)
        .join(format!("{id}.delivered"));
    if delivered_path.is_file() {
        return Ok(EnqueueResult {
            id,
            already_delivered: true,
        });
    }

    let created_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_millis()
        .try_into()
        .context("report queue timestamp exceeds u64")?;
    let record = QueuedReport {
        version: QUEUE_VERSION,
        id: id.clone(),
        created_at_ms,
        event: event.to_owned(),
        provider_id: provider_id.to_owned(),
        hook_body,
    };
    let queue_path = state_directory
        .join(QUEUE_DIRECTORY)
        .join(format!("{id}.json"));
    if queue_path.is_file() {
        return Ok(EnqueueResult {
            id,
            already_delivered: false,
        });
    }
    write_private_file(&queue_path, &serde_json::to_vec(&record)?)?;
    Ok(EnqueueResult {
        id,
        already_delivered: false,
    })
}

pub(super) fn load_pending_at(state_directory: &Path) -> anyhow::Result<Vec<QueuedReport>> {
    let queue_directory = state_directory.join(QUEUE_DIRECTORY);
    if !queue_directory.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in fs::read_dir(&queue_directory)
        .with_context(|| format!("read DUCX report queue {}", queue_directory.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let record: QueuedReport = serde_json::from_slice(&fs::read(entry.path())?)
            .with_context(|| format!("parse queued DUCX report {}", entry.path().display()))?;
        ensure!(
            record.version == QUEUE_VERSION,
            "unsupported DUCX report queue version {} in {}",
            record.version,
            entry.path().display()
        );
        ensure!(
            entry.file_name().to_string_lossy() == format!("{}.json", record.id),
            "DUCX report queue filename does not match its record ID: {}",
            entry.path().display()
        );
        let delivered_path = state_directory
            .join(DELIVERED_DIRECTORY)
            .join(format!("{}.delivered", record.id));
        if delivered_path.is_file() {
            fs::remove_file(entry.path()).with_context(|| {
                format!(
                    "remove already-delivered DUCX report {}",
                    entry.path().display()
                )
            })?;
            continue;
        }
        records.push(record);
    }
    records.sort_by_key(|record| {
        let event_priority = match record.event.as_str() {
            "user-prompt-submit" => 0,
            "pre-tool-use" => 1,
            "post-tool-use" => 2,
            "stop" => 3,
            _ => 4,
        };
        (event_priority, record.created_at_ms)
    });
    Ok(records)
}

pub(super) fn mark_delivered_at(
    state_directory: &Path,
    record: &QueuedReport,
) -> anyhow::Result<()> {
    let delivered_path = state_directory
        .join(DELIVERED_DIRECTORY)
        .join(format!("{}.delivered", record.id));
    write_private_file(&delivered_path, b"")?;
    let queue_path = state_directory
        .join(QUEUE_DIRECTORY)
        .join(format!("{}.json", record.id));
    match fs::remove_file(&queue_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("remove delivered DUCX report {}", queue_path.display())),
    }
}

pub(super) fn lock_at(state_directory: &Path) -> anyhow::Result<File> {
    fs::create_dir_all(state_directory)?;
    let lock_path = state_directory.join("ducx-report-queue.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open DUCX report queue lock {}", lock_path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("lock DUCX report queue {}", lock_path.display()))?;
    Ok(lock)
}

fn write_private_file(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    write_atomic_if_changed(path, contents)
        .with_context(|| format!("write DUCX report queue state {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
