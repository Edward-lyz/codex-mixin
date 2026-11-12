//! Native PTY/ConPTY transport for the unmodified official Claude executable.
//! Blocking terminal I/O stays on owned threads; dropping the handle ends them.
use std::io::{Read, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use portable_pty::{ChildKiller, CommandBuilder, PtySize, native_pty_system};
use serde_json::{Value, json};

pub struct InteractiveChild {
    stopped: Arc<AtomicBool>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl Drop for InteractiveChild {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.killer.kill();
    }
}

pub type InteractiveInput = tokio::sync::mpsc::Sender<Vec<Value>>;

/// The command must describe an interactive invocation, never print mode.
/// `history` holds Messages API messages the session resumes from, so only
/// new input goes through the terminal.
pub fn spawn(
    command: &std::process::Command,
    history: &[Value],
) -> anyhow::Result<(
    InteractiveChild,
    InteractiveInput,
    tokio::sync::mpsc::Receiver<Value>,
)> {
    let temp = tempfile::Builder::new().prefix("mixin-claude-").tempdir()?;
    let tap = temp.path().join("claude-tap.js");
    let events_file = temp.path().join("events.jsonl");
    super::write_owner_only(&tap, include_bytes!("claude_tap.js"))?;
    super::write_owner_only(&events_file, b"")?;
    let mut builder = CommandBuilder::from_argv(
        std::iter::once(command.get_program().to_owned())
            .chain(command.get_args().map(ToOwned::to_owned))
            .collect(),
    );
    for (name, value) in command.get_envs() {
        match value {
            Some(value) => {
                builder.env(name, value);
            }
            None => {
                builder.env_remove(name);
            }
        }
    }
    let options = std::env::var("BUN_OPTIONS").unwrap_or_default();
    // Bun's preload option has no portable quoting for whitespace in paths.
    anyhow::ensure!(
        !tap.to_string_lossy().contains(char::is_whitespace),
        "interactive Claude temporary path contains whitespace"
    );
    builder.env(
        "BUN_OPTIONS",
        format!("{options} --preload {}", tap.display()),
    );
    builder.env("CODEX_MIXIN_CLAUDE_TAP", &events_file);
    builder.env("TERM", "xterm-256color");
    if let Some(cwd) = command.get_current_dir() {
        builder.cwd(cwd);
    }
    let session_id = uuid::Uuid::new_v4().to_string();
    let config_root = command
        .get_envs()
        .find_map(|(name, value)| (name == "CLAUDE_CONFIG_DIR").then_some(value).flatten())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from))
        .unwrap_or_else(|| super::home_dir().join(".claude"));
    let pair = native_pty_system().openpty(PtySize {
        rows: 50,
        cols: 200,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut resumed = None;
    if history.is_empty() {
        builder.args(["--session-id", &session_id]);
    } else {
        // Claude Code looks up resumed sessions in the project folder named
        // after its resolved working directory, each non-alphanumeric UTF-16
        // unit a dash.
        let cwd = command
            .get_current_dir()
            .map(std::path::Path::to_path_buf)
            .map_or_else(std::env::current_dir, Ok)?
            .canonicalize()?;
        let cwd_text = cwd.to_string_lossy();
        let cwd_text = cwd_text.strip_prefix(r"\\?\").unwrap_or(&cwd_text);
        let project: String = cwd_text
            .encode_utf16()
            .map(|unit| match char::from_u32(unit.into()) {
                Some(c) if c.is_ascii_alphanumeric() => c,
                _ => '-',
            })
            .collect();
        let folder = config_root.join("projects").join(project);
        super::ensure_owner_only_dir(&folder)?;
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let mut parent = Value::Null;
        let mut transcript = Vec::new();
        for message in history {
            let id = uuid::Uuid::new_v4().to_string();
            let line = json!({
                "parentUuid": parent,
                "isSidechain": false,
                "type": message["role"],
                "message": message,
                "uuid": id,
                "timestamp": timestamp,
                "sessionId": session_id,
                "cwd": cwd_text,
                "userType": "external",
            });
            serde_json::to_writer(&mut transcript, &line)?;
            transcript.push(b'\n');
            parent = Value::String(id);
        }
        let file = folder.join(format!("{session_id}.jsonl"));
        super::write_owner_only(&file, &transcript)?;
        builder.args(["--resume", &session_id]);
        resumed = Some(file);
    }
    let mut child = match pair.slave.spawn_command(builder) {
        Ok(child) => child,
        Err(error) => {
            // The worker that sweeps this run's transcript never starts.
            if let Some(file) = resumed {
                let _ = std::fs::remove_file(file);
            }
            return Err(error.context("start interactive Claude PTY"));
        }
    };
    let killer = child.clone_killer();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
    let screen = Arc::new(Mutex::new(vt100::Parser::new(50, 200, 0)));
    let terminal = screen.clone();
    let reply_writer = writer.clone();
    std::thread::spawn(move || {
        let mut bytes = [0; 8192];
        while let Ok(size) = reader.read(&mut bytes) {
            if size == 0 {
                break;
            }
            if let Ok(mut parser) = terminal.lock() {
                parser.process(&bytes[..size]);
            }
            if bytes[..size].windows(4).any(|part| part == b"\x1b[6n")
                && let Ok(mut writer) = reply_writer.lock()
            {
                let _ = writer.write_all(b"\x1b[1;1R");
                let _ = writer.flush();
            }
        }
    });
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let (input, mut incoming) = tokio::sync::mpsc::channel::<Vec<Value>>(8);
    let (sender, events) = tokio::sync::mpsc::channel(512);
    std::thread::spawn(move || {
        // These owners must outlive the terminal and its streaming files.
        let _master = pair.master;
        let result = (|| -> anyhow::Result<()> {
            let mut stream = std::fs::File::open(&events_file)?;
            let mut pending = Vec::new();
            let mut ready = false;
            let mut tap_ready = false;
            let mut trust_answered = false;
            let mut last_dismiss: Option<Instant> = None;
            let mut turn_stop = String::new();
            let start = Instant::now();
            loop {
                if stop.load(Ordering::Acquire) || sender.is_closed() {
                    return Ok(());
                }
                if child.try_wait()?.is_some() {
                    bail!("interactive Claude exited; no headless fallback is enabled");
                }
                anyhow::ensure!(
                    stream.metadata()?.len() <= 128 * 1024 * 1024,
                    "interactive Claude capture exceeds 128 MiB; start a new session"
                );
                let mut chunk = Vec::new();
                std::io::Read::by_ref(&mut stream)
                    .take(8 * 1024 * 1024 + 1)
                    .read_to_end(&mut chunk)?;
                pending.extend_from_slice(&chunk);
                anyhow::ensure!(
                    pending.len() <= 8 * 1024 * 1024,
                    "interactive Claude event exceeds 8 MiB"
                );
                while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                    let line = pending[..end].to_owned();
                    pending.drain(..=end);
                    let event: Value = serde_json::from_slice(&line)?;
                    if let Some(tap) = event["tap"].as_str() {
                        anyhow::ensure!(
                            tap == "on",
                            "this Claude runtime does not support interactive streaming"
                        );
                        tap_ready = true;
                        continue;
                    }
                    if let Some(limits) = event.get("rate") {
                        if sender
                            .blocking_send(json!({"type":"rate_limits", "headers": limits}))
                            .is_err()
                        {
                            return Ok(());
                        }
                        continue;
                    }
                    if event["event"]["type"] == "message_delta" {
                        turn_stop = event["event"]["delta"]["stop_reason"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned();
                    }
                    let finished =
                        event["event"]["type"] == "message_stop" && turn_stop != "tool_use";
                    if sender.blocking_send(event).is_err() {
                        return Ok(());
                    }
                    if finished {
                        let _ = sender.blocking_send(json!({"type":"result", "is_error":false}));
                        turn_stop.clear();
                    }
                }
                let display = screen
                    .lock()
                    .map(|parser| parser.screen().contents())
                    .unwrap_or_default();
                // Only the gateway-owned working folder is trusted here.
                let trust_prompt = display.contains("Yes, I trust this folder");
                if !trust_answered && trust_prompt {
                    std::thread::sleep(Duration::from_millis(350));
                    let mut output = writer
                        .lock()
                        .map_err(|_| anyhow::anyhow!("terminal writer poisoned"))?;
                    output.write_all(b"\x1b[B\r")?;
                    output.flush()?;
                    trust_answered = true;
                }
                // Option menus such as the fullscreen renderer offer also draw
                // the prompt glyph, and pasted input would land in the menu.
                // Pasted text that starts with a digit starts on a new line,
                // so a user message never puts a digit right after the glyph.
                let menu = display.lines().any(|line| {
                    line.trim_start()
                        .strip_prefix('\u{276f}')
                        .is_some_and(|rest| {
                            rest.trim_start().starts_with(|c: char| c.is_ascii_digit())
                        })
                });
                if menu
                    && !trust_prompt
                    && last_dismiss.is_none_or(|at| at.elapsed() > Duration::from_secs(1))
                {
                    let mut output = writer
                        .lock()
                        .map_err(|_| anyhow::anyhow!("terminal writer poisoned"))?;
                    output.write_all(b"\x1b")?;
                    output.flush()?;
                    last_dismiss = Some(Instant::now());
                }
                if !menu && display.contains('\u{276f}') {
                    ready = true;
                }
                anyhow::ensure!(
                    (ready && tap_ready) || start.elapsed() < Duration::from_secs(45),
                    "interactive Claude did not reach its prompt within 45s; finish onboarding in Claude Code first"
                );
                if ready
                    && tap_ready
                    && !menu
                    && let Ok(content) = incoming.try_recv()
                {
                    for (index, part) in content.iter().enumerate() {
                        if let Some(words) = part["text"].as_str() {
                            paste(&writer, &screen, words, false, index != 0, &stop)?;
                        } else if part["type"] == "image" {
                            use base64::Engine;
                            let data = part["source"]["data"]
                                .as_str()
                                .context("interactive image must contain base64 data")?;
                            let extension = match part["source"]["media_type"].as_str() {
                                Some("image/jpeg") => "jpg",
                                Some("image/webp") => "webp",
                                Some("image/gif") => "gif",
                                Some("image/png") => "png",
                                _ => bail!("unsupported interactive image media type"),
                            };
                            let image = temp.path().join(format!("image-{index}.{extension}"));
                            super::write_owner_only(
                                &image,
                                &base64::engine::general_purpose::STANDARD.decode(data)?,
                            )?;
                            paste(
                                &writer,
                                &screen,
                                &image.to_string_lossy(),
                                true,
                                false,
                                &stop,
                            )?;
                        } else {
                            bail!("unsupported interactive input content");
                        }
                    }
                    let mut output = writer
                        .lock()
                        .map_err(|_| anyhow::anyhow!("terminal writer poisoned"))?;
                    output.write_all(b"\r")?;
                    output.flush()?;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        })();
        let _ = child.kill();
        let _ = child.wait();
        // Only this run's native transcript is ours; never sweep other sessions.
        for entry in walkdir::WalkDir::new(config_root.join("projects"))
            .max_depth(2)
            .into_iter()
            .flatten()
        {
            if entry.file_name() == std::ffi::OsStr::new(&format!("{session_id}.jsonl")) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        if let Err(error) = result {
            let _ = sender
                .try_send(json!({"type":"result", "is_error":true, "result":error.to_string()}));
        }
    });
    Ok((InteractiveChild { stopped, killer }, input, events))
}

/// Wait for the terminal to acknowledge each piece before submitting the turn.
/// Images are pasted separately so Claude loads them rather than reading paths as text.
fn paste(
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    screen: &Arc<Mutex<vt100::Parser>>,
    text: &str,
    image: bool,
    separate: bool,
    stopped: &AtomicBool,
) -> anyhow::Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    // Escape terminal controls from untrusted prompts; an embedded paste-end
    // sequence must never become keystrokes in the interactive CLI.
    let safe: String = text
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                format!("\\u{:04x}", character as u32)
            } else {
                character.to_string()
            }
        })
        .collect();
    let text = safe.as_str();
    let before = screen
        .lock()
        .map_err(|_| anyhow::anyhow!("terminal screen poisoned"))?
        .screen()
        .contents();
    {
        let mut writer = writer
            .lock()
            .map_err(|_| anyhow::anyhow!("terminal writer poisoned"))?;
        writer.write_all(b"\x1b[200~")?;
        // Separate later blocks without changing the first ordinary text block.
        // A leading command marker or digit also needs a newline so the CLI
        // does not interpret it as a command or an option menu.
        if !image
            && (separate
                || text
                    .trim_start()
                    .starts_with(|c: char| matches!(c, '/' | '!' | '&') || c.is_ascii_digit()))
        {
            writer.write_all(b"\n")?;
        }
        writer.write_all(text.as_bytes())?;
        writer.write_all(b"\x1b[201~")?;
        writer.flush()?;
    }
    // The terminal wraps and indents inline pastes, so compare without any
    // whitespace; a tail ending in a line break would otherwise never match.
    let tail = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .rev()
        .take(20)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    // Folded paste markers can wrap anywhere, including before '#' or within
    // their number. Compare both screens without terminal whitespace.
    let before: String = before
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let marker = if image { "[Image#" } else { "[Pastedtext#" };
    let previous = before
        .split(marker)
        .skip(1)
        .filter_map(|part| {
            let number: String = part.chars().take_while(char::is_ascii_digit).collect();
            number.parse::<u64>().ok()
        })
        .max()
        .unwrap_or_default();
    let started = Instant::now();
    let timeout = Duration::from_secs(5 + (text.len() / 65536).min(55) as u64);
    loop {
        let now = screen
            .lock()
            .map_err(|_| anyhow::anyhow!("terminal screen poisoned"))?
            .screen()
            .contents();
        let compact: String = now
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let taken = compact
            .split(marker)
            .skip(1)
            .filter_map(|part| {
                let number: String = part.chars().take_while(char::is_ascii_digit).collect();
                number.parse::<u64>().ok()
            })
            .max()
            .unwrap_or_default();
        if taken > previous || (!image && compact != before && compact.contains(&tail)) {
            std::thread::sleep(Duration::from_millis(100));
            return Ok(());
        }
        anyhow::ensure!(
            !stopped.load(Ordering::Acquire),
            "interactive Claude was cancelled"
        );
        anyhow::ensure!(
            started.elapsed() < timeout,
            "interactive Claude did not acknowledge the pasted content; screen: {}",
            now.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ")
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}
