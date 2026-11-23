//! In-app updates, following the Wails v3 updater that Magpie ships: find the
//! release on GitHub, download one archive holding the whole app, verify an
//! Ed25519 signature over its SHA-256 digest, stage it beside the installed
//! app, then let a helper process swap it in after the app quits.
//!
//! The helper is the newly downloaded binary started with
//! `CODEX_MIXIN_APPLY_UPDATE`, so the swap never runs code from the version
//! being replaced.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::store::query;

/// The app version, shared with the CLI workspace (set by build.rs).
pub const APP_VERSION: &str = env!("CODEX_MIXIN_VERSION");
const RELEASES_URL: &str = "https://github.com/Edward-lyz/codex-mixin/releases";
/// Replaces the GitHub releases URL, for networks that reach GitHub through a
/// mirror. Downloads are still verified against the embedded key.
const MIRROR_ENV: &str = "CODEX_MIXIN_UPDATE_MIRROR";
/// Raw Ed25519 public key that release archives are signed with.
const UPDATE_PUBLIC_KEY: &str = "7/xoN6K5aLjia8jZCIL7UAYXlWg5thAo6vTkVcWcDRU=";
const HELPER_ENV: &str = "CODEX_MIXIN_APPLY_UPDATE";
const STAGING_PREFIX: &str = ".codex-mixin-update-";
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const PARENT_EXIT_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[cfg(target_os = "macos")]
const APP_EXECUTABLE: &str = "Contents/MacOS/codex-mixin-desktop";
#[cfg(windows)]
const APP_EXECUTABLE: &str = "Codex Mixin.exe";
#[cfg(not(any(target_os = "macos", windows)))]
const APP_EXECUTABLE: &str = "codex-mixin-desktop";

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
    },
    Downloading {
        version: String,
        progress: Option<f32>,
    },
    Ready {
        version: String,
    },
    Failed {
        message: SharedString,
    },
}

#[derive(Deserialize)]
struct UpdateCheck {
    latest_version: String,
}

/// What the helper needs to finish an update after the app has quit.
#[derive(Serialize, Deserialize)]
struct HelperPlan {
    parent_pid: u32,
    /// The installed app: the `.app` bundle on macOS, the app folder on Windows.
    target: PathBuf,
    /// The verified, extracted replacement for `target`.
    source: PathBuf,
}

pub struct Updater {
    pub state: UpdateState,
    /// The installed app, or `None` for development builds that cannot update.
    installed: Option<PathBuf>,
    staged: Option<PathBuf>,
    _schedule: Task<()>,
}

struct GlobalUpdater(Entity<Updater>);

impl Global for GlobalUpdater {}

/// Compares dotted numeric versions; anything unparsable compares as zero.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |version: &str| -> Vec<u64> {
        version
            .split(['.', '-'])
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    parse(latest) > parse(current)
}

impl Updater {
    pub fn init(cx: &mut App) {
        let updater = cx.new(|cx| {
            let schedule = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FIRST_CHECK_DELAY).await;
                loop {
                    let checked = this.update(cx, |updater: &mut Updater, cx| updater.check(cx));
                    if checked.is_err() {
                        break;
                    }
                    cx.background_executor().timer(CHECK_INTERVAL).await;
                }
            });
            Self {
                state: UpdateState::Idle,
                installed: installed_app(),
                staged: None,
                _schedule: schedule,
            }
        });
        cx.set_global(GlobalUpdater(updater));
    }

    pub fn global(cx: &App) -> Entity<Updater> {
        cx.global::<GlobalUpdater>().0.clone()
    }

    /// Asks the CLI for the latest release. A staged or downloading update
    /// is left alone.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.state,
            UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Ready { .. }
        ) {
            return;
        }
        self.state = UpdateState::Checking;
        cx.notify();
        let task =
            query::<UpdateCheck>(vec!["update".into(), "--check".into(), "--json".into()], cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |updater, cx| {
                updater.state = match result {
                    Ok(check) if is_newer(&check.latest_version, APP_VERSION) => {
                        UpdateState::Available {
                            version: check.latest_version,
                        }
                    }
                    Ok(_) => UpdateState::UpToDate,
                    Err(error) => UpdateState::Failed {
                        message: error.message.into(),
                    },
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Downloads, verifies and stages the available update.
    pub fn download(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available { version } = self.state.clone() else {
            return;
        };
        let Some(installed) = self.installed.clone() else {
            self.state = UpdateState::Failed {
                message: rust_i18n::t!("update.not_packaged").into(),
            };
            cx.notify();
            return;
        };
        self.state = UpdateState::Downloading {
            version: version.clone(),
            progress: None,
        };
        cx.notify();
        let (progress_sender, progress_receiver) = smol::channel::unbounded::<f32>();
        let (result_sender, result_receiver) = smol::channel::bounded::<anyhow::Result<PathBuf>>(1);
        let worker_version = version.clone();
        thread::spawn(move || {
            let result = stage_update(&worker_version, &installed, &progress_sender);
            let _ = result_sender.try_send(result);
        });
        cx.spawn(async move |this, cx| {
            while let Ok(progress) = progress_receiver.recv().await {
                let version = version.clone();
                let updated = this.update(cx, |updater, cx| {
                    updater.state = UpdateState::Downloading {
                        version,
                        progress: Some(progress),
                    };
                    cx.notify();
                });
                if updated.is_err() {
                    return;
                }
            }
            let result = result_receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("the update worker stopped")));
            this.update(cx, |updater, cx| {
                match result {
                    Ok(staged) => {
                        updater.staged = Some(staged);
                        updater.state = UpdateState::Ready {
                            version: version.clone(),
                        };
                    }
                    Err(error) => {
                        updater.state = UpdateState::Failed {
                            message: format!("{error:#}").into(),
                        };
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts the helper from the staged app and quits; the helper swaps the
    /// app in once this process has exited and launches the new version.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        let (Some(source), Some(target)) = (self.staged.clone(), self.installed.clone()) else {
            return;
        };
        let plan = HelperPlan {
            parent_pid: std::process::id(),
            target,
            source: source.clone(),
        };
        let spawned = serde_json::to_string(&plan)
            .map_err(anyhow::Error::from)
            .and_then(|plan| {
                Command::new(source.join(APP_EXECUTABLE))
                    .env(HELPER_ENV, plan)
                    .spawn()
                    .map_err(anyhow::Error::from)
            });
        match spawned {
            Ok(_) => cx.quit(),
            Err(error) => {
                self.state = UpdateState::Failed {
                    message: format!("{error:#}").into(),
                };
                cx.notify();
            }
        }
    }
}

/// The installed app containing the running executable.
fn installed_app() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let app = if cfg!(target_os = "macos") {
        executable.ancestors().nth(3)?.to_path_buf()
    } else {
        executable.parent()?.to_path_buf()
    };
    // A Linux build output folder also holds `codex-mixin-desktop`; only the
    // packaged folder ships the installer beside it.
    let packaged = app.join(APP_EXECUTABLE).is_file()
        && (!cfg!(target_os = "macos")
            || app.extension().is_some_and(|extension| extension == "app"))
        && (!cfg!(target_os = "linux") || app.join("install.sh").is_file());
    packaged.then_some(app)
}

fn release_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", _) => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", _) => "x86_64-unknown-linux-gnu",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        _ => "x86_64-pc-windows-msvc",
    }
}

/// The release asset holding the whole app for this platform. Linux uses a
/// tarball because GNU tar, which extracts it, cannot read zip files.
pub fn archive_name(version: &str) -> String {
    let extension = if cfg!(target_os = "linux") {
        "tar.gz"
    } else {
        "zip"
    };
    format!("codex-mixin-app-{version}-{}.{extension}", release_target())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into()
}

/// Downloads the archive and its signature, verifies them and extracts the
/// app into a staging folder beside the installed one.
fn stage_update(
    version: &str,
    installed: &Path,
    progress: &smol::channel::Sender<f32>,
) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-')),
        "the release version {version} is invalid"
    );
    let parent = installed
        .parent()
        .ok_or_else(|| anyhow::anyhow!("the installed app has no parent folder"))?;
    let staging = parent.join(format!("{STAGING_PREFIX}{version}"));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging).map_err(|error| {
        anyhow::anyhow!(
            "{}: {error}",
            rust_i18n::t!("update.not_writable", path = parent.display().to_string())
        )
    })?;
    let name = archive_name(version);
    let agent = agent();
    let releases = std::env::var(MIRROR_ENV)
        .ok()
        .filter(|mirror| mirror.starts_with("https://") || mirror.starts_with("http://127.0.0.1"))
        .unwrap_or_else(|| RELEASES_URL.to_owned());
    let releases = releases.trim_end_matches('/');
    let signature = agent
        .get(&format!("{releases}/download/v{version}/{name}.sig"))
        .call()?
        .body_mut()
        .read_to_string()?;
    let archive = staging.join(&name);
    let digest = download(
        &agent,
        &format!("{releases}/download/v{version}/{name}"),
        &archive,
        progress,
    )?;
    verify_signature(&digest, signature.trim())?;
    let extracted = staging.join("app");
    fs::create_dir_all(&extracted)?;
    extract(&archive, &extracted)?;
    let app = single_entry(&extracted)?;
    anyhow::ensure!(
        app.join(APP_EXECUTABLE).is_file(),
        "the update archive does not contain the app"
    );
    verify_app(&app, installed)?;
    Ok(app)
}

/// Streams the download to disk and returns its SHA-256 digest.
fn download(
    agent: &ureq::Agent,
    url: &str,
    destination: &Path,
    progress: &smol::channel::Sender<f32>,
) -> anyhow::Result<[u8; 32]> {
    let mut response = agent.get(url).call()?;
    let total = response.body().content_length();
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_ARCHIVE_BYTES)
        .reader();
    let mut file = fs::File::create(destination)?;
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut reported = Instant::now();
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        file.write_all(&buffer[..count])?;
        received += count as u64;
        if let Some(total) = total.filter(|_| reported.elapsed() > Duration::from_millis(100)) {
            let _ = progress.try_send(received as f32 / total.max(1) as f32);
            reported = Instant::now();
        }
    }
    file.sync_all()?;
    Ok(hasher.finalize().into())
}

fn verify_signature(digest: &[u8; 32], signature: &str) -> anyhow::Result<()> {
    let engine = base64::engine::general_purpose::STANDARD;
    let key: [u8; 32] = engine
        .decode(UPDATE_PUBLIC_KEY)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("the update public key is malformed"))?;
    let signature: [u8; 64] = engine
        .decode(signature)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("the update signature is malformed"))?;
    VerifyingKey::from_bytes(&key)?
        .verify(digest, &Signature::from_bytes(&signature))
        .map_err(|_| anyhow::anyhow!("{}", rust_i18n::t!("update.bad_signature")))
}

fn run(command: &mut Command, what: &str) -> anyhow::Result<String> {
    let output = command.output()?;
    anyhow::ensure!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Uses the system archiver: `ditto` keeps macOS code signatures intact, and
/// Windows 10 and later ship `tar` with zip support, and Linux archives are
/// tarballs.
fn extract(archive: &Path, destination: &Path) -> anyhow::Result<()> {
    if cfg!(target_os = "macos") {
        run(
            Command::new("ditto")
                .arg("-x")
                .arg("-k")
                .arg(archive)
                .arg(destination),
            "ditto",
        )?;
    } else {
        run(
            Command::new("tar")
                .arg("-xf")
                .arg(archive)
                .arg("-C")
                .arg(destination),
            "tar",
        )?;
    }
    Ok(())
}

/// The archive must hold exactly one top-level entry, which replaces the
/// installed app as a whole.
fn single_entry(directory: &Path) -> anyhow::Result<PathBuf> {
    let mut entries = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path());
    let first = entries
        .next()
        .ok_or_else(|| anyhow::anyhow!("the update archive is empty"))?;
    anyhow::ensure!(
        entries.next().is_none(),
        "the update archive has more than one top-level entry"
    );
    Ok(first)
}

/// On macOS the new bundle must carry a valid signature for the same bundle
/// identifier as the installed app.
fn verify_app(app: &Path, installed: &Path) -> anyhow::Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    run(
        Command::new("codesign")
            .arg("--verify")
            .arg("--deep")
            .arg("--strict")
            .arg(app),
        "codesign",
    )?;
    let identifier = |bundle: &Path| {
        run(
            Command::new("plutil")
                .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
                .arg(bundle.join("Contents/Info.plist")),
            "plutil",
        )
    };
    anyhow::ensure!(
        identifier(app)? == identifier(installed)?,
        "the update is for a different app"
    );
    Ok(())
}

/// Runs the helper when this process was started to apply an update.
/// Returns `false` for a normal launch.
pub fn run_helper_if_requested() -> bool {
    let Ok(plan) = std::env::var(HELPER_ENV) else {
        return false;
    };
    let result = serde_json::from_str::<HelperPlan>(&plan)
        .map_err(anyhow::Error::from)
        .and_then(|plan| apply(&plan).map(|()| plan));
    match result {
        Ok(plan) => relaunch(&plan.target),
        Err(error) => eprintln!("cannot apply the update: {error:#}"),
    }
    true
}

fn apply(plan: &HelperPlan) -> anyhow::Result<()> {
    wait_for_exit(plan.parent_pid)?;
    if cfg!(windows) {
        swap_files(&plan.source, &plan.target)
    } else {
        swap_bundle(&plan.source, &plan.target)
    }
}

fn process_running(pid: u32) -> bool {
    if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")))
            .unwrap_or(false)
    } else {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}

fn wait_for_exit(pid: u32) -> anyhow::Result<()> {
    let started = Instant::now();
    while process_running(pid) {
        anyhow::ensure!(
            started.elapsed() < PARENT_EXIT_TIMEOUT,
            "the running app did not quit"
        );
        thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// Replaces a macOS bundle by renaming; the old bundle is restored if the
/// new one cannot be moved into place.
fn swap_bundle(source: &Path, target: &Path) -> anyhow::Result<()> {
    let aside = target.with_extension(format!("app.old-{}", std::process::id()));
    fs::rename(target, &aside)?;
    if let Err(error) = fs::rename(source, target) {
        fs::rename(&aside, target)?;
        return Err(error.into());
    }
    let _ = fs::remove_dir_all(&aside);
    Ok(())
}

/// Windows cannot rename a folder while a program inside it runs (the
/// gateway keeps the CLI open), but it can rename the running files
/// themselves. Each file is moved aside and replaced; leftovers are removed
/// on the next launch.
fn swap_files(source: &Path, target: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        if destination.exists() {
            let aside =
                destination.with_file_name(format!("{}.old", entry.file_name().to_string_lossy()));
            if aside.is_dir() {
                fs::remove_dir_all(&aside)?;
            } else if aside.exists() {
                fs::remove_file(&aside)?;
            }
            fs::rename(&destination, &aside)?;
        }
        fs::rename(entry.path(), &destination)?;
    }
    Ok(())
}

fn relaunch(target: &Path) {
    let launched = if cfg!(target_os = "macos") {
        Command::new("open").arg("-n").arg(target).spawn()
    } else {
        // The helper ran with the update plan in its environment; the
        // relaunched app must start normally.
        Command::new(target.join(APP_EXECUTABLE))
            .env_remove(HELPER_ENV)
            .spawn()
    };
    if let Err(error) = launched {
        eprintln!("cannot relaunch the updated app: {error}");
    }
}

/// Removes what an applied update left behind: the staging folder and, on
/// Windows, the files moved aside while they were running.
pub fn clean_up_after_update() {
    let Some(installed) = installed_app() else {
        return;
    };
    if let Some(parent) = installed.parent()
        && let Ok(entries) = fs::read_dir(parent)
    {
        for entry in entries.filter_map(Result::ok) {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(STAGING_PREFIX)
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
    if cfg!(windows)
        && let Ok(entries) = fs::read_dir(&installed)
    {
        for entry in entries.filter_map(Result::ok) {
            if entry.file_name().to_string_lossy().ends_with(".old") {
                let _ = fs::remove_file(entry.path()).or_else(|_| fs::remove_dir_all(entry.path()));
            }
        }
    }
}
