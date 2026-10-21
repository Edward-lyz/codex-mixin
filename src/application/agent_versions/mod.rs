//! Version detection and upgrades for every fixed agent in magpie's All().
//! Dynamic WSL and omp profile rows are managed by their own discovery paths.
//! Only source-verified install sources offer upgrades; other rows explicitly
//! report unavailable rather than guessing a package or executing a shell.
//!
//! Detection reads local state only: where a CLI is, how it was installed,
//! what version it reports, and the version of a desktop app installed beside
//! it. The newest version comes from the source that would update it (npm or
//! Homebrew); when that cannot be read, `latest` stays `None` and `error`
//! says why, so an unknown version is never shown as up to date. One agent is
//! upgraded at a time, in this process and across processes. Version and
//! newest-version answers are cached in this process only, so a fresh CLI
//! invocation asks the local sources again and stays true of the machine as
//! it is now.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Context;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::platform;
use crate::platform::agent_cli::{AgentUpdateLock, DesktopApp, NpmGlobalInstall};

/// Timeout of one CLI version probe.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a version a CLI reported is trusted while its binary is
/// unchanged.
const VERSION_CACHE_OK: Duration = Duration::from_secs(24 * 60 * 60);
/// How long a failed version probe is remembered.
const VERSION_CACHE_FAILED: Duration = Duration::from_secs(10 * 60);
/// How long a newest version from npm or Homebrew is trusted.
const LATEST_CACHE_OK: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a failed newest-version read is remembered.
const LATEST_CACHE_FAILED: Duration = Duration::from_secs(15 * 60);
/// Timeout of one request to the npm registry.
const NPM_TIMEOUT: Duration = Duration::from_secs(8);
/// Timeout of one Homebrew version query.
const BREW_TIMEOUT: Duration = Duration::from_secs(15);
/// Timeout of one CLI upgrade, which downloads a release.
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Where npm's packages are read.
const NPM_REGISTRY: &str = "https://registry.npmjs.org/";

/// Agents the version core covers, in the order [`check_versions`] reports
/// them. The ids match the Agents page.
pub const AGENT_VERSION_IDS: [&str; 50] = [
    "codex",
    "claude",
    "claude-desktop",
    "dsh",
    "opencode",
    "pi",
    "gemini",
    "agy",
    "openchamber",
    "mimocode",
    "aside",
    "omo",
    "goose",
    "cursor",
    "cursor-local",
    "zed",
    "vscode",
    "vscode-insiders",
    "vscodium",
    "copilot-jetbrains",
    "air",
    "copilot",
    "crush",
    "reasonix",
    "commandcode",
    "fx",
    "omp",
    "devin",
    "hermes",
    "morph",
    "kimi",
    "qwen",
    "muse",
    "empryo",
    "ante",
    "minimax-code",
    "droid",
    "cline",
    "qoder",
    "qoder-cn",
    "grok",
    "zcode",
    "workbuddy",
    "codebuddy",
    "pencil",
    "t3code",
    "hanako",
    "atomcode",
    "alma",
    "cindy",
];

/// One agent's CLI state, as the Agents page shows it.
///
/// `None` means unknown, which is not "up to date": when the newest version
/// cannot be read, `latest` stays `None` and `error` says why. A row with no
/// `path`, no `version`, and no `error` is an agent that is not installed.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentCliInfo {
    /// Version the installed CLI reports.
    pub version: Option<String>,
    /// Newest version where this CLI would be updated from.
    pub latest: Option<String>,
    /// What updates it: `npm`, `bun`, `pnpm`, `brew`, or `self`.
    pub via: Option<String>,
    /// Where the CLI was found.
    pub path: Option<String>,
    /// The update as a person would type it.
    pub command: Option<String>,
    /// Version of the desktop app installed beside the CLI, when the agent
    /// has one.
    pub app_version: Option<String>,
    /// `latest` is after `version` and this machine can install it.
    pub update: bool,
    /// Why something could not be read or run; every reason found, oldest
    /// first, joined by `; `. `None` when everything asked for succeeded.
    pub error: Option<String>,
}

/// One row of [`check_versions`]. `Default` is what serde fills a missing
/// field of a row with.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentVersionEntry {
    pub id: String,
    pub cli: AgentCliInfo,
}

/// Where a CLI is published: its npm packages (the current one first), its
/// Homebrew formulae or casks, its vendor installer's own updater, and the
/// desktop app that carries it.
struct AgentSpec {
    id: &'static str,
    /// Command name on `PATH`; `None` for an agent that is an app alone.
    command: Option<&'static str>,
    /// Vendor installer folder under the home directory, where the CLI lives
    /// when `PATH` has no command of that name (forward slashes).
    home_bin: Option<&'static str>,
    npm: &'static [&'static str],
    brew: &'static [&'static str],
    self_update: Option<SelfUpdate>,
    desktop_app: Option<DesktopApp>,
}

/// A CLI its vendor's own installer put in place, which updates itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelfUpdate {
    /// Path fragment of the installer's location, with forward slashes.
    marker: &'static str,
    /// The subcommand that updates it.
    verb: &'static str,
    /// Windows keeps the launcher where the installer put it, without a link
    /// into the versioned folder, so `~/.local/bin` names the install.
    home_launcher: bool,
}

/// Fixed All() ids and Bin values from internal/agent/agents.go and the
/// individual constructors. Upgrade packages and verbs are restricted to
/// internal/agent/cliupdate.go; a missing source is deliberately unavailable.
const SPECS: [AgentSpec; 50] = [
    AgentSpec {
        id: "codex",
        command: Some("codex"),
        home_bin: Some(".local/bin"),
        npm: &["@openai/codex"],
        brew: &["codex"],
        self_update: Some(SelfUpdate {
            marker: "/.codex/packages/standalone/",
            verb: "update",
            home_launcher: false,
        }),
        desktop_app: Some(DesktopApp::Codex),
    },
    AgentSpec {
        id: "claude",
        command: Some("claude"),
        home_bin: Some(".local/bin"),
        npm: &["@anthropic-ai/claude-code"],
        brew: &["claude-code"],
        self_update: Some(SelfUpdate {
            marker: "/.local/share/claude/versions/",
            verb: "update",
            home_launcher: true,
        }),
        desktop_app: None,
    },
    AgentSpec {
        id: "claude-desktop",
        command: None,
        home_bin: None,
        npm: &[],
        brew: &[],
        self_update: None,
        desktop_app: Some(DesktopApp::ClaudeDesktop),
    },
    AgentSpec {
        id: "dsh",
        command: Some("dsh"),
        home_bin: None,
        npm: &["@deepseek-ai/dsh"],
        brew: &[],
        self_update: None,
        desktop_app: Some(DesktopApp::Dsh),
    },
    AgentSpec {
        id: "opencode",
        command: Some("opencode"),
        home_bin: Some(".opencode/bin"),
        npm: &["opencode-ai"],
        brew: &["opencode"],
        self_update: Some(SelfUpdate {
            marker: "/.opencode/bin/",
            verb: "upgrade",
            home_launcher: false,
        }),
        desktop_app: None,
    },
    AgentSpec {
        id: "pi",
        command: Some("pi"),
        home_bin: None,
        npm: &[
            "@earendil-works/pi-coding-agent",
            "@mariozechner/pi-coding-agent",
        ],
        brew: &["pi-coding-agent"],
        self_update: None,
        desktop_app: None,
    },
    agent_spec(
        "gemini",
        Some("gemini"),
        &["@google/gemini-cli"],
        &["gemini-cli"],
    ),
    agent_spec("agy", Some("agy"), &[], &[]),
    agent_spec("openchamber", Some("openchamber"), &[], &[]),
    agent_spec("mimocode", Some("mimo"), &[], &[]),
    agent_spec("aside", Some("aside"), &[], &[]),
    AgentSpec {
        self_update: Some(SelfUpdate {
            // Magpie uses omo update for every install to move its pinned engine.
            marker: "",
            verb: "update",
            home_launcher: false,
        }),
        ..agent_spec("omo", Some("omo"), &["omo-ai"], &[])
    },
    agent_spec("goose", Some("goose"), &[], &["block-goose-cli"]),
    agent_spec("cursor", Some("cursor-agent"), &[], &[]),
    agent_spec("cursor-local", None, &[], &[]),
    agent_spec("zed", Some("zed"), &[], &[]),
    agent_spec("vscode", Some("code"), &[], &[]),
    agent_spec("vscode-insiders", Some("code-insiders"), &[], &[]),
    agent_spec("vscodium", Some("codium"), &[], &[]),
    agent_spec("copilot-jetbrains", None, &[], &[]),
    // These constructors omit Bin to avoid colliding with unrelated tools.
    agent_spec("air", None, &[], &[]),
    agent_spec(
        "copilot",
        Some("copilot"),
        &["@github/copilot"],
        &["copilot-cli"],
    ),
    agent_spec("crush", Some("crush"), &["@charmland/crush"], &["crush"]),
    agent_spec("reasonix", Some("reasonix"), &[], &[]),
    agent_spec("commandcode", Some("command-code"), &[], &[]),
    agent_spec("fx", None, &[], &[]),
    agent_spec("omp", Some("omp"), &["@oh-my-pi/pi-coding-agent"], &[]),
    agent_spec("devin", Some("devin"), &[], &[]),
    agent_spec("hermes", Some("hermes"), &[], &[]),
    agent_spec("morph", None, &[], &[]),
    agent_spec("kimi", Some("kimi"), &[], &[]),
    agent_spec("qwen", Some("qwen"), &[], &[]),
    agent_spec("muse", Some("muse"), &[], &[]),
    agent_spec("empryo", Some("empryo"), &[], &[]),
    agent_spec("ante", Some("ante"), &[], &[]),
    agent_spec("minimax-code", Some("mcode"), &[], &[]),
    agent_spec("droid", Some("droid"), &[], &[]),
    agent_spec("cline", Some("cline"), &["cline"], &[]),
    agent_spec("qoder", Some("qodercli"), &[], &[]),
    agent_spec("qoder-cn", Some("qoderclicn"), &[], &[]),
    agent_spec("grok", None, &[], &[]),
    agent_spec("zcode", None, &[], &[]),
    agent_spec("workbuddy", None, &[], &[]),
    agent_spec(
        "codebuddy",
        Some("codebuddy"),
        &["@tencent-ai/codebuddy-code"],
        &[],
    ),
    agent_spec("pencil", None, &[], &[]),
    agent_spec("t3code", None, &[], &[]),
    agent_spec("hanako", None, &[], &[]),
    agent_spec(
        "atomcode",
        Some("atomcode"),
        &["@atomgit.com/atomcode"],
        &["atomcode"],
    ),
    agent_spec("alma", None, &[], &[]),
    agent_spec("cindy", None, &[], &[]),
];

/// Common static spec shape; empty package lists mean no verified updater.
const fn agent_spec(
    id: &'static str,
    command: Option<&'static str>,
    npm: &'static [&'static str],
    brew: &'static [&'static str],
) -> AgentSpec {
    AgentSpec {
        id,
        command,
        home_bin: None,
        npm,
        brew,
        self_update: None,
        desktop_app: None,
    }
}

fn spec_for(id: &str) -> Option<&'static AgentSpec> {
    SPECS.iter().find(|spec| spec.id == id)
}

/// Where an installed CLI is updated from.
#[derive(Clone, Debug, Eq, PartialEq)]
enum LatestSource {
    Npm(String),
    Brew { package: String, cask: bool },
}

/// How an installed CLI is brought up to date.
#[derive(Clone, Debug)]
struct Updater {
    via: &'static str,
    /// The executable to run.
    program: PathBuf,
    /// Its arguments, already split; the command is never built as a shell
    /// string.
    args: Vec<String>,
    latest: LatestSource,
    /// Why this user cannot run the update, when that is known before
    /// running it (an npm global folder only an administrator can change).
    blocked: Option<String>,
}

impl Updater {
    /// The update as a person would type it.
    fn display_command(&self) -> String {
        let program = self
            .program
            .file_name()
            .unwrap_or_else(|| self.program.as_os_str());
        std::iter::once(program.to_string_lossy().into_owned())
            .chain(self.args.iter().cloned())
            .map(|part| {
                if part.contains([' ', '\t']) {
                    platform::shell_quote(&part)
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn latest_cache_key(&self) -> String {
        match &self.latest {
            LatestSource::Npm(package) => format!("latest:npm:{package}"),
            LatestSource::Brew { package, cask } => {
                let kind = if *cask { "cask" } else { "formula" };
                format!("latest:brew:{kind}:{package}")
            }
        }
    }
}

/// What one agent looks like on this machine right now.
struct LocalState {
    info: AgentCliInfo,
    updater: Option<Updater>,
}

/// Local state of one agent's CLI: version, install source, update command,
/// and the desktop app version beside it. No network, no update; `latest`
/// stays `None` and `update` false. Fails only for an unknown `id`.
pub fn discover_agent(id: &str) -> anyhow::Result<AgentCliInfo> {
    let spec = spec_for(id).ok_or_else(|| unknown_agent(id))?;
    Ok(inspect_local(spec).info)
}

/// Every agent's installed and newest version, in [`AGENT_VERSION_IDS`]
/// order. `refresh` asks the sources again instead of answering from cache.
/// A per-agent failure, including a failing inspection worker, is reported
/// in that row's `error`; the call itself does not fail.
pub async fn check_versions(refresh: bool) -> Vec<AgentVersionEntry> {
    if refresh {
        cache().clear();
    }
    let inspections = AGENT_VERSION_IDS.iter().copied().map(inspect_entry);
    futures_util::future::join_all(inspections).await
}

async fn inspect_entry(id: &'static str) -> AgentVersionEntry {
    let Some(spec) = spec_for(id) else {
        return AgentVersionEntry {
            id: id.to_owned(),
            cli: AgentCliInfo {
                error: Some(format!("unknown agent id: {id}")),
                ..AgentCliInfo::default()
            },
        };
    };
    let state = match tokio::task::spawn_blocking(move || inspect_local(spec)).await {
        Ok(state) => state,
        Err(error) => {
            return AgentVersionEntry {
                id: id.to_owned(),
                cli: AgentCliInfo {
                    error: Some(format!("inspect {id} failed: {error}")),
                    ..AgentCliInfo::default()
                },
            };
        }
    };
    let LocalState { mut info, updater } = state;
    if let Some(updater) = &updater {
        let answer = latest_version(updater).await;
        apply_latest(&mut info, updater, answer);
    }
    AgentVersionEntry {
        id: id.to_owned(),
        cli: info,
    }
}

/// Update one agent's CLI the way it was installed, then read its version
/// again. Fails only for an unknown `id`; a refusal (no CLI, an install
/// source that says nothing sure, a folder only an administrator can change,
/// an update already running) or a failing update is reported in the
/// returned `error`.
pub async fn upgrade_agent(id: &str) -> anyhow::Result<AgentCliInfo> {
    let spec = spec_for(id).ok_or_else(|| unknown_agent(id))?;
    let state = tokio::task::spawn_blocking(move || inspect_local(spec))
        .await
        .context("agent inspection worker failed")?;
    let LocalState {
        info: before,
        updater,
    } = state;
    let Some(updater) = updater else {
        let message = refusal(id, before.path.is_some());
        return Ok(refused(before, message));
    };
    if let Some(reason) = updater.blocked.clone() {
        return Ok(refused(before, reason));
    }
    let Some(_in_process) = InProcessUpgrade::acquire(id) else {
        return Ok(refused(before, format!("{id} is already being updated")));
    };
    let _process_lock = match AgentUpdateLock::try_acquire(id) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            return Ok(refused(
                before,
                format!("another process is already updating {id}"),
            ));
        }
        Err(error) => return Ok(refused(before, format!("{error:#}"))),
    };
    let version_before = before.version.clone();
    let outcome = run_updater(&updater).await;
    if let Some(path) = before.path.as_deref() {
        cache().forget(&version_cache_key(Path::new(path)));
    }
    if outcome.is_ok() && updater.via == "brew" {
        // brew upgrade updates Homebrew itself first, which may know newer.
        cache().forget(&updater.latest_cache_key());
    }
    let mut info = tokio::task::spawn_blocking(move || inspect_local(spec))
        .await
        .context("agent inspection worker failed")?
        .info;
    if let Err(error) = &outcome {
        push_error(&mut info, format!("{error:#}"));
    }
    let answer = latest_version(&updater).await;
    apply_latest(&mut info, &updater, answer);
    // A clean exit that left the old version is still a failed upgrade.
    if outcome.is_ok()
        && info.update
        && version_before == info.version
        && let Some(after) = info.version.clone()
    {
        let command = updater.display_command();
        let latest = info.latest.as_deref().unwrap_or("a newer version");
        let message = format!("{id} is still {after} after {command}; {latest} is out");
        push_error(&mut info, message);
    }
    Ok(info)
}

/// `info` with `message` added to its error.
fn refused(mut info: AgentCliInfo, message: String) -> AgentCliInfo {
    push_error(&mut info, message);
    info
}

/// Why no update is offered: the CLI is absent, or where it came from says
/// nothing sure.
fn refusal(id: &str, cli_found: bool) -> String {
    if cli_found {
        format!("cannot tell how {id} was installed; update it the way you installed it")
    } else {
        format!("{id}'s CLI was not found on PATH or in its installer's folder")
    }
}

fn unknown_agent(id: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "unknown agent id: {id}; expected one of {}",
        AGENT_VERSION_IDS.join(", ")
    )
}

/// Add `message` to `info`'s error, keeping earlier context.
fn push_error(info: &mut AgentCliInfo, message: String) {
    match &mut info.error {
        Some(existing) => {
            existing.push_str("; ");
            existing.push_str(&message);
        }
        None => info.error = Some(message),
    }
}

/// What one agent's CLI is on this machine, without asking the network.
fn inspect_local(spec: &AgentSpec) -> LocalState {
    let app_version = spec.desktop_app.and_then(desktop_app_version);
    match spec.command.and_then(|command| locate_cli(spec, command)) {
        Some(bin) => inspect_cli(spec, bin, app_version),
        None => LocalState {
            info: AgentCliInfo {
                app_version,
                error: if spec.command.is_none() {
                    Some(format!(
                        "{} CLI version detection and upgrade unavailable: no verified command",
                        spec.id
                    ))
                } else if spec.npm.is_empty() && spec.brew.is_empty() {
                    Some(format!(
                        "{} upgrade unavailable: no verified updater",
                        spec.id
                    ))
                } else {
                    None
                },
                ..AgentCliInfo::default()
            },
            updater: None,
        },
    }
}

/// What the CLI at `bin` is, with the desktop app's version already read.
fn inspect_cli(spec: &AgentSpec, bin: PathBuf, app_version: Option<String>) -> LocalState {
    let mut info = AgentCliInfo {
        app_version,
        path: Some(bin.to_string_lossy().into_owned()),
        ..AgentCliInfo::default()
    };
    let updater = detect_updater(spec, &bin);
    if let Some(updater) = &updater {
        info.via = Some(updater.via.to_owned());
        info.command = Some(updater.display_command());
        if let Some(reason) = &updater.blocked {
            push_error(&mut info, reason.clone());
        }
    } else {
        push_error(
            &mut info,
            format!(
                "{} upgrade unavailable: no verified updater for this install",
                spec.id
            ),
        );
    }
    let probe = installed_version(&bin);
    info.version = probe.value;
    if let Some(error) = probe.error {
        push_error(&mut info, error);
    }
    LocalState { info, updater }
}

/// Where the CLI is: on `PATH` first, then the vendor installer's own
/// location under the home folder, which a terminal opened now may not have
/// on `PATH`.
fn locate_cli(spec: &AgentSpec, command: &str) -> Option<PathBuf> {
    if let Some(bin) = platform::agent_cli::find_command(command) {
        return Some(bin);
    }
    let home = platform::home_dir_required().ok()?;
    let candidate = home
        .join(spec.home_bin?)
        .join(platform::executable_file_name(command));
    candidate.is_file().then_some(candidate)
}

/// Version of a desktop app installed on this machine, from the first
/// location that names one.
fn desktop_app_version(app: DesktopApp) -> Option<String> {
    platform::agent_cli::desktop_app_candidates(app)
        .into_iter()
        .find_map(|location| {
            platform::agent_cli::desktop_app_versions(&location)
                .into_iter()
                .reduce(|best, version| {
                    if newer(&version, &best) {
                        version
                    } else {
                        best
                    }
                })
        })
}

/// How `bin` was installed and would be updated, from where its links lead;
/// `None` when that says nothing sure, in which case no update is offered.
fn detect_updater(spec: &AgentSpec, bin: &Path) -> Option<Updater> {
    let real = platform::agent_cli::real_executable(bin);
    if let Some(updater) = self_updater(spec, bin, &real) {
        return Some(updater);
    }
    if let Some(updater) = brew_updater(spec, &real) {
        return Some(updater);
    }
    for package in spec.npm {
        if let Some(updater) = node_updater(package, bin, &real) {
            return Some(updater);
        }
    }
    None
}

/// The updater of a CLI the vendor's own installer put in place: its own
/// `update` (or `upgrade`) subcommand, with the newest version read from its
/// npm package.
fn self_updater(spec: &AgentSpec, bin: &Path, real: &Path) -> Option<Updater> {
    let self_update = spec.self_update?;
    let installed = slash_text(real).contains(self_update.marker)
        || (self_update.home_launcher
            && platform::agent_cli::is_local_bin_launcher(real, spec.command?));
    if !installed {
        return None;
    }
    Some(Updater {
        via: "self",
        program: bin.to_path_buf(),
        args: vec![self_update.verb.to_owned()],
        latest: LatestSource::Npm((*spec.npm.first()?).to_owned()),
        blocked: None,
    })
}

/// The updater of a CLI that lives in Homebrew's Cellar or Caskroom.
fn brew_updater(spec: &AgentSpec, real: &Path) -> Option<Updater> {
    let (prefix, package, cask) = brew_install(real)?;
    if !spec.brew.contains(&package.as_str()) {
        return None;
    }
    let local = prefix.join("bin").join("brew");
    let brew = if local.is_file() {
        local
    } else {
        platform::agent_cli::find_command("brew")?
    };
    let mut args = vec!["upgrade".to_owned()];
    if cask {
        args.push("--cask".to_owned());
    }
    args.push(package.clone());
    Some(Updater {
        via: "brew",
        program: brew,
        args,
        latest: LatestSource::Brew { package, cask },
        blocked: None,
    })
}

/// Homebrew's prefix, formula or cask name, and whether it is a cask, from a
/// path inside Homebrew's Cellar or Caskroom.
fn brew_install(real: &Path) -> Option<(PathBuf, String, bool)> {
    let text = slash_text(real);
    for (marker, cask) in [("/Cellar/", false), ("/Caskroom/", true)] {
        let Some(start) = text.find(marker) else {
            continue;
        };
        let rest = &text[start + marker.len()..];
        let name = rest.split('/').next().filter(|name| !name.is_empty())?;
        return Some((PathBuf::from(&text[..start]), name.to_owned(), cask));
    }
    None
}

/// The updater of a CLI a Node package manager installed globally: its
/// binary leads into a global `node_modules`, or is a launcher script that
/// names one.
fn node_updater(package: &str, bin: &Path, real: &Path) -> Option<Updater> {
    let real_text = slash_text(real);
    let mark = format!("/node_modules/{package}/");
    // A manager's path alone is not proof that this is the requested package.
    // pnpm shims can name a store elsewhere, so identity and location differ.
    if !real_text.contains(&mark) && !shim_names(bin, package) {
        return None;
    }
    let modules = real_text
        .find(&mark)
        .map(|index| PathBuf::from(&real_text[..index]).join("node_modules"))
        .or_else(|| shim_modules(bin, package));
    let lower = format!("{real_text}|{}", slash_text(bin)).to_lowercase();
    if lower.contains("/.bun/") {
        let local = bin.parent()?.join(platform::executable_file_name("bun"));
        let bun = if local.is_file() {
            local
        } else {
            platform::agent_cli::find_command("bun")?
        };
        return Some(global_add_updater("bun", bun, package));
    }
    if lower.contains("/pnpm/") || lower.contains("/.pnpm/") {
        let pnpm = platform::agent_cli::find_command("pnpm")?;
        return Some(global_add_updater("pnpm", pnpm, package));
    }
    if lower.contains("/.volta/") || lower.contains("/yarn/") || lower.contains("/.yarn/") {
        // A version manager of its own keeps these; updating them behind its
        // back would leave its links wrong.
        return None;
    }
    let install = platform::agent_cli::npm_global_install(&modules?)?;
    Some(npm_updater(package, install))
}

/// `<manager> add -g <package>@latest`, which bun and pnpm share.
fn global_add_updater(via: &'static str, program: PathBuf, package: &str) -> Updater {
    Updater {
        via,
        program,
        args: vec![
            "add".to_owned(),
            "-g".to_owned(),
            format!("{package}@latest"),
        ],
        latest: LatestSource::Npm(package.to_owned()),
        blocked: None,
    }
}

/// The npm updater of a global install, refused before it runs when npm's
/// global folder is one only an administrator can change.
fn npm_updater(package: &str, install: NpmGlobalInstall) -> Updater {
    let blocked = if platform::agent_cli::directory_writable(&install.prefix) {
        None
    } else {
        Some(format!(
            "npm's global folder {} is not writable by this user; run the update in a terminal that can write there, or move npm's global folder",
            install.prefix.display()
        ))
    };
    Updater {
        via: "npm",
        program: install.npm,
        args: vec![
            "install".to_owned(),
            "-g".to_owned(),
            "--prefix".to_owned(),
            install.prefix.to_string_lossy().into_owned(),
            format!("{package}@latest"),
        ],
        latest: LatestSource::Npm(package.to_owned()),
        blocked,
    }
}

/// The `node_modules` folder a launcher script names, for npm's Windows
/// `.cmd` shims and pnpm's scripts, whose own path does not lead into it.
fn shim_modules(bin: &Path, package: &str) -> Option<PathBuf> {
    if !shim_names(bin, package) {
        return None;
    }
    let modules = bin.parent()?.join("node_modules");
    modules.join(package).is_dir().then_some(modules)
}

fn shim_names(bin: &Path, package: &str) -> bool {
    /// A launcher script is small; anything larger is a real binary.
    const SHIM_MAX_BYTES: u64 = 64 * 1024;
    let Ok(metadata) = std::fs::metadata(bin) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > SHIM_MAX_BYTES {
        return false;
    }
    std::fs::read_to_string(bin).is_ok_and(|text| {
        text.replace('\\', "/")
            .contains(&format!("node_modules/{package}/"))
    })
}

/// `path` with forward slashes, which both Windows and Unix patterns match.
fn slash_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Fill in the newest version and the update flag from an answer about the
/// source that would update this CLI.
fn apply_latest(info: &mut AgentCliInfo, updater: &Updater, answer: CachedAnswer) {
    if let Some(value) = answer.value {
        info.latest = Some(value);
    }
    if let Some(error) = answer.error {
        push_error(info, error);
    }
    if updater.blocked.is_none()
        && let (Some(version), Some(latest)) = (info.version.as_deref(), info.latest.as_deref())
    {
        info.update = newer(latest, version);
    }
}

/// One cached answer: the value and the error of the last ask.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CachedAnswer {
    value: Option<String>,
    error: Option<String>,
}

struct CacheEntry {
    stamp: String,
    answer: CachedAnswer,
    expires: Instant,
}

/// Answers that took a process or a request to get, kept per key for as long
/// as this process lives. Concurrent misses for the same key may ask twice;
/// the answers are the same.
#[derive(Default)]
struct Cache {
    entries: Mutex<HashMap<String, CacheEntry>>,
}

impl Cache {
    /// The cached answer for `key` while it is fresh and `stamp` is
    /// unchanged.
    fn get(&self, key: &str, stamp: &str) -> Option<CachedAnswer> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = entries.get(key)?;
        (entry.stamp == stamp && entry.expires > Instant::now()).then(|| entry.answer.clone())
    }

    /// The last answer for `key` and `stamp`, even after it went stale: a
    /// failure keeps the version it last knew instead of dropping it.
    fn last(&self, key: &str, stamp: &str) -> Option<CachedAnswer> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries
            .get(key)
            .filter(|entry| entry.stamp == stamp)
            .map(|entry| entry.answer.clone())
    }

    fn store(&self, key: &str, stamp: &str, answer: CachedAnswer, ttl: Duration) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.insert(
            key.to_owned(),
            CacheEntry {
                stamp: stamp.to_owned(),
                answer,
                expires: Instant::now() + ttl,
            },
        );
    }

    /// Store the outcome of one ask and return it as an answer. A failure
    /// keeps the last value known for the same `stamp`.
    fn record(
        &self,
        key: &str,
        stamp: &str,
        outcome: anyhow::Result<String>,
        ok_ttl: Duration,
        failed_ttl: Duration,
    ) -> CachedAnswer {
        let (answer, ttl) = match outcome {
            Ok(version) => (
                CachedAnswer {
                    value: Some(version),
                    error: None,
                },
                ok_ttl,
            ),
            Err(error) => (
                CachedAnswer {
                    value: self.last(key, stamp).and_then(|answer| answer.value),
                    error: Some(format!("{error:#}")),
                },
                failed_ttl,
            ),
        };
        self.store(key, stamp, answer.clone(), ttl);
        answer
    }

    fn forget(&self, key: &str) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.remove(key);
    }

    fn clear(&self) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.clear();
    }
}

/// Answers about versions and newest versions, shared by every caller in
/// this process.
fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Cache::default)
}

/// What the CLI at `bin` says its version is, asked again only once the
/// binary it runs changes.
fn installed_version(bin: &Path) -> CachedAnswer {
    let real = platform::agent_cli::real_executable(bin);
    // Match magpie internal/proc/version.go: npm metadata does not start the
    // CLI, and is reread so an in-place package update cannot leave it stale.
    if let Some(version) = npm_installed_version(&real) {
        return CachedAnswer {
            value: Some(version),
            error: None,
        };
    }
    let stamp = binary_stamp(&real);
    let key = version_cache_key(bin);
    if let Some(answer) = cache().get(&key, &stamp) {
        return answer;
    }
    let probe =
        platform::agent_cli::probe_version_output(bin, VERSION_PROBE_TIMEOUT).and_then(|output| {
            parse_version(&output)
                .ok_or_else(|| anyhow::anyhow!("{} printed no version", bin.display()))
        });
    // A failure keeps the version of the same binary; a changed binary
    // starts empty, so a stale version is never shown as new.
    cache().record(&key, &stamp, probe, VERSION_CACHE_OK, VERSION_CACHE_FAILED)
}

/// Version of the enclosing npm package, never an unrelated parent project.
fn npm_installed_version(real: &Path) -> Option<String> {
    for directory in real.parent()?.ancestors() {
        let Some(parent) = directory.parent() else {
            continue;
        };
        let directly_in_modules = parent
            .file_name()
            .is_some_and(|name| name == "node_modules");
        let scoped_in_modules = parent
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('@'))
            && parent
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "node_modules");
        if directly_in_modules || scoped_in_modules {
            let content = std::fs::read_to_string(directory.join("package.json")).ok()?;
            let package: serde_json::Value = serde_json::from_str(&content).ok()?;
            return package
                .get("version")
                .and_then(serde_json::Value::as_str)
                .and_then(parse_version);
        }
    }
    None
}

fn version_cache_key(bin: &Path) -> String {
    format!("version:{}", bin.to_string_lossy())
}

/// Identity of the binary a launcher runs: a new release is a new path, a
/// new size, or a new time.
fn binary_stamp(real: &Path) -> String {
    match std::fs::metadata(real) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            format!("{}|{}|{modified}", real.display(), metadata.len())
        }
        Err(_) => format!("{}|missing", real.display()),
    }
}

/// Newest version where `updater` would update from, with the error of the
/// ask that could not be answered.
async fn latest_version(updater: &Updater) -> CachedAnswer {
    let key = updater.latest_cache_key();
    if let Some(answer) = cache().get(&key, "") {
        return answer;
    }
    let fetched = match &updater.latest {
        LatestSource::Npm(package) => npm_latest(package).await,
        LatestSource::Brew { package, cask } => {
            brew_latest(updater.program.clone(), package.clone(), *cask).await
        }
    };
    cache().record(&key, "", fetched, LATEST_CACHE_OK, LATEST_CACHE_FAILED)
}

/// Newest version of an npm package, the registry's `latest` tag.
async fn npm_latest(package: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::builder().timeout(NPM_TIMEOUT).build()?;
    let response = client
        .get(format!("{NPM_REGISTRY}{package}/latest"))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .with_context(|| format!("query npm for {package}"))?
        .error_for_status()
        .with_context(|| format!("query npm for {package}"))?;
    let body: serde_json::Value = response
        .json()
        .await
        .with_context(|| format!("read npm's answer for {package}"))?;
    let version = body
        .get("version")
        .and_then(serde_json::Value::as_str)
        .filter(|version| !version.is_empty())
        .ok_or_else(|| anyhow::anyhow!("npm names no latest version for {package}"))?;
    Ok(version.to_owned())
}

/// Homebrew's `info --json=v2` answer, the parts a version comes from.
#[derive(Deserialize)]
struct BrewInfo {
    #[serde(default)]
    formulae: Vec<BrewFormula>,
    #[serde(default)]
    casks: Vec<BrewCask>,
}

#[derive(Deserialize)]
struct BrewFormula {
    versions: BrewFormulaVersions,
}

#[derive(Deserialize)]
struct BrewFormulaVersions {
    stable: Option<String>,
}

#[derive(Deserialize)]
struct BrewCask {
    version: Option<String>,
}

/// Newest version Homebrew has for `package`, as it last updated itself.
async fn brew_latest(brew: PathBuf, package: String, cask: bool) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || brew_latest_blocking(&brew, &package, cask))
        .await
        .context("Homebrew version worker failed")?
}

fn brew_latest_blocking(brew: &Path, package: &str, cask: bool) -> anyhow::Result<String> {
    let mut command = platform::launcher_command(brew);
    command.args(["info", "--json=v2"]);
    if cask {
        command.arg("--cask");
    }
    command
        .arg(package)
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1");
    let what = format!("{} info {package}", brew.display());
    let outcome = platform::agent_cli::run_command(command, BREW_TIMEOUT, &what)?;
    brew_info_version(&outcome.stdout, package)
}

/// The version in Homebrew's `info --json=v2` output.
fn brew_info_version(stdout: &str, package: &str) -> anyhow::Result<String> {
    let info: BrewInfo =
        serde_json::from_str(stdout).with_context(|| format!("parse brew info for {package}"))?;
    let raw = if let Some(formula) = info.formulae.first() {
        formula.versions.stable.clone()
    } else if let Some(cask) = info.casks.first() {
        // A cask's version may carry a build: 1.2.3,build
        cask.version
            .as_deref()
            .and_then(|version| version.split(',').next())
            .map(str::to_owned)
    } else {
        None
    };
    raw.as_deref()
        .and_then(parse_version)
        .ok_or_else(|| anyhow::anyhow!("Homebrew names no version for {package}"))
}

/// Run the updater with nothing to answer it, bounded by
/// [`UPGRADE_TIMEOUT`].
async fn run_updater(updater: &Updater) -> anyhow::Result<()> {
    let mut command = platform::launcher_command(&updater.program);
    command
        .args(&updater.args)
        .current_dir(platform::home_dir())
        .env("NONINTERACTIVE", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .env("NO_COLOR", "1");
    let what = updater.display_command();
    tokio::task::spawn_blocking(move || {
        platform::agent_cli::run_command(command, UPGRADE_TIMEOUT, &what).map(|_| ())
    })
    .await
    .context("agent upgrade worker failed")?
}

/// One upgrade per agent inside this process.
struct InProcessUpgrade(String);

fn running_upgrades() -> &'static Mutex<HashSet<String>> {
    static RUNNING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(HashSet::new()))
}

impl InProcessUpgrade {
    /// Hold the in-process slot for `agent_id`, or `None` while another
    /// caller in this process is already updating it.
    fn acquire(agent_id: &str) -> Option<Self> {
        let mut running = running_upgrades()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        running
            .insert(agent_id.to_owned())
            .then(|| Self(agent_id.to_owned()))
    }
}

impl Drop for InProcessUpgrade {
    fn drop(&mut self) {
        let mut running = running_upgrades()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        running.remove(&self.0);
    }
}

/// The first version in what a CLI printed; a color code may precede it.
fn parse_version(output: &str) -> Option<String> {
    static VERSION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?:^|[^\w.]|\x1b\[[0-9;]*m)(?:v)?(\d+\.\d+(?:\.\d+)*(?:-[0-9A-Za-z][0-9A-Za-z.]*)?)",
        )
        .expect("valid version pattern")
    });
    let captures = VERSION_PATTERN.captures(output)?;
    let version = captures.get(1)?.as_str().trim_end_matches('.');
    (!version.is_empty()).then(|| version.to_owned())
}

/// Whether version `a` is after `b`: by their numbers, then a release after
/// its pre-releases (1.0.87 after 1.0.87-0).
fn newer(a: &str, b: &str) -> bool {
    let (a_core, a_pre) = split_prerelease(a.trim_start_matches('v'));
    let (b_core, b_pre) = split_prerelease(b.trim_start_matches('v'));
    let a_parts: Vec<&str> = a_core.split('.').collect();
    let b_parts: Vec<&str> = b_core.split('.').collect();
    for index in 0..a_parts.len().max(b_parts.len()) {
        let left = a_parts.get(index).map_or(0, |part| parse_number(part));
        let right = b_parts.get(index).map_or(0, |part| parse_number(part));
        if left != right {
            return left > right;
        }
    }
    match (a_pre, b_pre) {
        (None, None) => false,
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (Some(left), Some(right)) => prerelease_newer(left, right),
    }
}

fn split_prerelease(version: &str) -> (&str, Option<&str>) {
    match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    }
}

fn parse_number(part: &str) -> u64 {
    part.parse::<u64>().unwrap_or(0)
}

/// Order pre-release tags part by part, numbers as numbers.
fn prerelease_newer(a: &str, b: &str) -> bool {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();
    for index in 0..a_parts.len().min(b_parts.len()) {
        let left = a_parts[index];
        let right = b_parts[index];
        match (left.parse::<u64>(), right.parse::<u64>()) {
            (Ok(left_number), Ok(right_number)) => {
                if left_number != right_number {
                    return left_number > right_number;
                }
            }
            _ => {
                if left != right {
                    return left > right;
                }
            }
        }
    }
    a_parts.len() > b_parts.len()
}

#[cfg(test)]
mod tests;
