//! Native desktop application registration. The OS owns application lookup;
//! these bridges only serialize its answers for the platform-neutral core.
//! No vendor program is launched to discover an application or its version.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use super::agent_cli::run_command;

/// Desktop products, independent of any command-line installation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesktopApp {
    Codex,
    ClaudeDesktop,
    Dsh,
    OpenCode,
    OpenChamber,
    Goose,
    Cursor,
    Zed,
    VsCode,
    VsCodeInsiders,
    VsCodium,
    Air,
    Reasonix,
    Qoder,
    QoderCn,
    ZCode,
    WorkBuddy,
    CodeBuddy,
    Pencil,
    T3Code,
    Hanako,
    Alma,
    Cindy,
}

impl DesktopApp {
    const ALL: &'static [Self] = &[
        Self::Codex,
        Self::ClaudeDesktop,
        Self::Dsh,
        Self::OpenCode,
        Self::OpenChamber,
        Self::Goose,
        Self::Cursor,
        Self::Zed,
        Self::VsCode,
        Self::VsCodeInsiders,
        Self::VsCodium,
        Self::Air,
        Self::Reasonix,
        Self::Qoder,
        Self::QoderCn,
        Self::ZCode,
        Self::WorkBuddy,
        Self::CodeBuddy,
        Self::Pencil,
        Self::T3Code,
        Self::Hanako,
        Self::Alma,
        Self::Cindy,
    ];

    fn names(self) -> &'static [&'static str] {
        match self {
            Self::Codex => &["Codex", "ChatGPT"],
            Self::ClaudeDesktop => &["Claude", "Claude Desktop"],
            Self::Dsh => &["DeepSeek Harness"],
            Self::OpenCode => &["OpenCode", "opencode"],
            Self::OpenChamber => &["OpenChamber"],
            Self::Goose => &["Goose", "goose"],
            Self::Cursor => &["Cursor"],
            Self::Zed => &["Zed", "Zed Preview"],
            Self::VsCode => &[
                "Visual Studio Code",
                "Microsoft Visual Studio Code",
                "Microsoft Visual Studio Code (User)",
            ],
            Self::VsCodeInsiders => &[
                "Visual Studio Code - Insiders",
                "Microsoft Visual Studio Code Insiders",
                "Microsoft Visual Studio Code Insiders (User)",
            ],
            Self::VsCodium => &["VSCodium", "VSCodium (User)"],
            Self::Air => &["Air", "JetBrains Air"],
            Self::Reasonix => &["Reasonix", "DeepSeek Reasonix"],
            Self::Qoder => &["Qoder"],
            Self::QoderCn => &["Qoder CN", "QoderCN"],
            Self::ZCode => &["ZCode"],
            Self::WorkBuddy => &["WorkBuddy"],
            Self::CodeBuddy => &["CodeBuddy"],
            Self::Pencil => &["Pencil"],
            Self::T3Code => &["T3 Code", "T3Code"],
            Self::Hanako => &["Hanako"],
            Self::Alma => &["Alma"],
            Self::Cindy => &["Cindy", "CindyGlobal"],
        }
    }

    fn identifiers(self) -> &'static [&'static str] {
        match self {
            Self::Codex => &[
                "com.openai.chat",
                "com.openai.codex",
                "OpenAI.ChatGPT-Desktop",
                "OpenAI.Codex",
            ],
            Self::ClaudeDesktop => &["com.anthropic.claudefordesktop", "Claude"],
            Self::Cursor => &["com.todesktop.230313mzl4w4u92", "cursor"],
            Self::Zed => &["dev.zed.Zed", "dev.zed.Zed-Preview", "zed"],
            Self::VsCode => &["com.microsoft.VSCode", "code"],
            Self::VsCodeInsiders => &["com.microsoft.VSCodeInsiders", "code-insiders"],
            Self::VsCodium => &["com.vscodium", "codium"],
            Self::Cindy => &[
                "com.xd.cindycn",
                "com.xd.cindyglobal",
                "cindy",
                "cindyglobal",
            ],
            // LaunchServices also resolves registered application names;
            // GIO and Windows expose the native display names directly.
            _ => &[],
        }
    }

    fn executables(self) -> &'static [&'static str] {
        match self {
            Self::VsCode => &["Code.exe"],
            Self::VsCodeInsiders => &["Code - Insiders.exe"],
            Self::VsCodium => &["VSCodium.exe"],
            Self::Dsh => &["DeepSeek Harness.exe"],
            Self::ClaudeDesktop => &["Claude.exe"],
            Self::Codex => &["Codex.exe", "ChatGPT.exe"],
            Self::Cindy => &["Cindy.exe", "CindyGlobal.exe"],
            Self::Cursor => &["Cursor.exe"],
            Self::Zed => &["zed.exe"],
            _ => &[],
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct NativeDesktopApp {
    app: DesktopApp,
    path: PathBuf,
    version: Option<String>,
}

#[derive(Default, Deserialize)]
struct NativeInventory {
    apps: Vec<NativeDesktopApp>,
    #[serde(default)]
    errors: Vec<String>,
}

/// A registered application can be installed even without version metadata.
/// The path is an application location, never a runnable CLI contract.
pub(crate) fn desktop_app_installation(
    app: DesktopApp,
) -> anyhow::Result<Option<(PathBuf, Option<String>)>> {
    let inventory = native_inventory()?;
    if let Some(entry) = inventory.apps.iter().find(|entry| entry.app == app) {
        return Ok(Some((entry.path.clone(), entry.version.clone())));
    }
    anyhow::ensure!(
        inventory.errors.is_empty(),
        "{}",
        inventory.errors.join("; ")
    );
    Ok(None)
}

/// Registered install locations, preferred by the native application resolver.
pub fn desktop_app_candidates(app: DesktopApp) -> Vec<PathBuf> {
    match native_inventory() {
        Ok(inventory) => inventory
            .apps
            .iter()
            .filter(|entry| entry.app == app)
            .map(|entry| entry.path.clone())
            .collect(),
        Err(error) => {
            tracing::debug!(%error, "native application discovery unavailable");
            Vec::new()
        }
    }
}

/// Only versions supplied by native application metadata; never execute a GUI.
pub fn desktop_app_versions(location: &Path) -> Vec<String> {
    native_inventory()
        .ok()
        .into_iter()
        .flat_map(|inventory| &inventory.apps)
        .filter(|entry| entry.path == location)
        .filter_map(|entry| entry.version.clone())
        .collect()
}

fn native_inventory() -> anyhow::Result<&'static NativeInventory> {
    // A CLI invocation is a snapshot. All 50 concurrent agent inspections share
    // one native query rather than spawning one inventory process per row.
    static INVENTORY: OnceLock<Result<NativeInventory, String>> = OnceLock::new();
    INVENTORY
        .get_or_init(|| query_native_inventory().map_err(|error| format!("{error:#}")))
        .as_ref()
        .map_err(|error| anyhow::anyhow!("{error}"))
}

fn query_native_inventory() -> anyhow::Result<NativeInventory> {
    let queries: Vec<_> = DesktopApp::ALL
        .iter()
        .map(|app| {
            serde_json::json!({
                "app": app, "names": app.names(), "identifiers": app.identifiers(),
                "executables": app.executables(),
            })
        })
        .collect();
    let mut command = native_inventory_command()?;
    command.env("CODEX_MIXIN_APP_QUERIES", serde_json::to_string(&queries)?);
    super::prepare_background_command(&mut command);
    let output = run_command(
        command,
        Duration::from_secs(10),
        "native application inventory",
    )?;
    read_native_inventory(&output.stdout)
}

fn read_native_inventory(stdout: &str) -> anyhow::Result<NativeInventory> {
    let mut inventory: NativeInventory =
        serde_json::from_str(stdout.trim()).context("invalid native application inventory")?;
    // Registration may outlive an uninstall. Do not promote stale entries or
    // missing metadata into a fabricated installed version.
    inventory
        .apps
        .retain(|entry| entry.path.is_absolute() && entry.path.exists());
    for entry in &mut inventory.apps {
        entry.version = entry
            .version
            .take()
            .filter(|value| !value.trim().is_empty());
    }
    Ok(inventory)
}

#[cfg(target_os = "macos")]
fn native_inventory_command() -> anyhow::Result<Command> {
    let mut command = Command::new("/usr/bin/osascript");
    command.args(["-l", "JavaScript", "-e", r#"
ObjC.import('AppKit');
var queries = JSON.parse(ObjC.unwrap($.NSProcessInfo.processInfo.environment.objectForKey('CODEX_MIXIN_APP_QUERIES')));
var workspace = $.NSWorkspace.sharedWorkspace;
var apps = [], errors = [];
queries.forEach(function(query) {
    var locations = [];
    query.identifiers.forEach(function(identifier) {
        var url = workspace.URLForApplicationWithBundleIdentifier(identifier);
        if (url && !url.isNil()) locations.push(ObjC.unwrap(url.path));
    });
    query.names.forEach(function(name) {
        var path = workspace.fullPathForApplication(name);
        if (path && !path.isNil()) locations.push(ObjC.unwrap(path));
    });
    locations.filter(function(path, index) { return locations.indexOf(path) === index; }).forEach(function(path) {
        var bundle = $.NSBundle.bundleWithPath(path);
        if (!bundle || bundle.isNil()) return;
        var value = bundle.objectForInfoDictionaryKey('CFBundleShortVersionString');
        apps.push({app:query.app, path:path, version:value && !value.isNil() ? ObjC.unwrap(value) : null});
    });
});
JSON.stringify({apps:apps, errors:errors});
"#]);
    Ok(command)
}

#[cfg(windows)]
fn native_inventory_command() -> anyhow::Result<Command> {
    // The Registry provider and Appx cmdlet call the native registration and
    // PackageManager APIs, including per-user/MSIX installs. No disk scan.
    let mut command = Command::new(super::system_powershell()?);
    command.args(["-NoProfile", "-NonInteractive", "-Command", r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$queries = ConvertFrom-Json $env:CODEX_MIXIN_APP_QUERIES
$apps = [System.Collections.Generic.List[object]]::new()
$errors = [System.Collections.Generic.List[string]]::new()
$roots = @('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall')
foreach ($root in $roots) {
    try {
        if (!(Test-Path -LiteralPath $root)) { continue }
        $keys = @(Get-ChildItem -LiteralPath $root)
    } catch { $errors.Add('Windows uninstall registration could not be enumerated'); continue }
    foreach ($key in $keys) {
        try {
            $item = Get-ItemProperty -LiteralPath $key.PSPath
        } catch { $errors.Add('A Windows uninstall registration could not be read'); continue }
        foreach ($query in $queries) { try {
            if ($query.names -notcontains $item.DisplayName) { continue }
            $location = [Environment]::ExpandEnvironmentVariables([string]$item.InstallLocation)
            $icon = [Environment]::ExpandEnvironmentVariables([string]$item.DisplayIcon)
            $icon = ($icon -replace ',\s*-?\d+$', '').Trim('"')
            $programs = @($query.executables) + @($query.names | ForEach-Object { "$_.exe" })
            $program = $null
            if ($icon -and (Test-Path -LiteralPath $icon -PathType Leaf) -and [IO.Path]::GetExtension($icon) -eq '.exe') {
                $program = $icon
            } elseif ($location) {
                foreach ($name in $programs) {
                    $candidate = Join-Path $location $name
                    if (Test-Path -LiteralPath $candidate -PathType Leaf) { $program = $candidate; break }
                }
            }
            if (!$program) { continue }
            $version = (Get-Item -LiteralPath $program).VersionInfo.ProductVersion
            if (!$version) { $version = [string]$item.DisplayVersion }
            $apps.Add(@{app=$query.app;path=$program;version=$version})
        } catch { $errors.Add('A registered Windows application could not be inspected') } }
    }
}
foreach ($root in @('HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths',
    'HKLM:\Software\Microsoft\Windows\CurrentVersion\App Paths',
    'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths')) {
    try {
        if (!(Test-Path -LiteralPath $root)) { continue }
    } catch { $errors.Add('Windows App Paths registration is unavailable'); continue }
    foreach ($query in $queries) {
        foreach ($name in $query.executables) { try {
            $key = Join-Path $root $name
            if (!(Test-Path -LiteralPath $key)) { continue }
            $program = [Environment]::ExpandEnvironmentVariables([string](Get-Item -LiteralPath $key).GetValue('')).Trim('"')
            if (!$program -or !(Test-Path -LiteralPath $program -PathType Leaf)) { continue }
            $version = (Get-Item -LiteralPath $program).VersionInfo.ProductVersion
            $apps.Add(@{app=$query.app;path=$program;version=$version})
        } catch { $errors.Add('A Windows App Paths registration could not be inspected') } }
    }
}
try {
    foreach ($package in Get-AppxPackage) {
        foreach ($query in $queries) { try {
            if (($query.identifiers -notcontains $package.Name) -and ($query.names -notcontains $package.Name)) { continue }
            if (!$package.InstallLocation -or !(Test-Path -LiteralPath $package.InstallLocation -PathType Container)) { continue }
            $apps.Add(@{app=$query.app;path=$package.InstallLocation;version=$package.Version.ToString()})
        } catch { $errors.Add('A Windows packaged application could not be inspected') } }
    }
} catch { $errors.Add('Windows packaged application registration is unavailable') }
ConvertTo-Json -Compress -Depth 5 @{apps=@($apps.ToArray());errors=@($errors.ToArray() | Sort-Object -Unique)}
"#]);
    Ok(command)
}

#[cfg(target_os = "linux")]
fn native_inventory_command() -> anyhow::Result<Command> {
    // GObject introspection keeps the musl CLI independent of a dynamically
    // linked GLib. The desktop's own GIO binding reads .desktop registrations.
    // No custom parser for desktop files, Exec quoting or XDG precedence.
    let mut command = Command::new("/usr/bin/python3");
    command.args(["-I", "-c", r#"
import json, os
try:
    import gi
    gi.require_version('Gio', '2.0')
    from gi.repository import Gio, GLib
except (ImportError, ValueError):
    print(json.dumps({'apps': [], 'errors': ['Native desktop discovery requires the system Python GIO binding (python3-gi)']}))
    raise SystemExit(0)
queries = json.loads(os.environ['CODEX_MIXIN_APP_QUERIES'])
apps = []
for app in Gio.AppInfo.get_all():
    if not isinstance(app, Gio.DesktopAppInfo):
        continue
    executable = app.get_executable()
    if not executable or not GLib.find_program_in_path(executable):
        continue
    identifier = app.get_id() or ''
    names = [app.get_name(), app.get_string('Name') or '']
    for query in queries:
        if not any(identifier == value or identifier == value + '.desktop' for value in query['identifiers']) and not any(name.casefold() == value.casefold() for name in names for value in query['names']):
            continue
        path = app.get_filename()
        if path:
            version = app.get_string('X-AppImage-Version') or app.get_string('X-Version')
            apps.append({'app': query['app'], 'path': path, 'version': version})
print(json.dumps({'apps': apps, 'errors': []}))
"#]);
    Ok(command)
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn native_inventory_command() -> anyhow::Result<Command> {
    anyhow::bail!("native desktop application registration is unavailable on this OS")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_without_a_version_still_identifies_an_installation() {
        let directory = tempfile::tempdir().unwrap();
        let location = directory.path().canonicalize().unwrap();
        let fixture = serde_json::json!({"apps": [{"app":"cindy", "path":location, "version":null}], "errors":[]});
        let inventory = read_native_inventory(&fixture.to_string()).unwrap();
        assert_eq!(inventory.apps.len(), 1);
        assert_eq!(inventory.apps[0].app, DesktopApp::Cindy);
        assert_eq!(inventory.apps[0].path, location);
        assert_eq!(inventory.apps[0].version, None);
    }

    #[test]
    fn stale_registrations_and_relative_paths_are_not_installations() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("uninstalled");
        let fixture = serde_json::json!({"apps": [
            {"app":"cindy", "path":missing, "version":"0.1.120"},
            {"app":"zed", "path":"relative", "version":null}
        ]});
        assert!(
            read_native_inventory(&fixture.to_string())
                .unwrap()
                .apps
                .is_empty()
        );
    }

    #[test]
    fn metadata_preserves_native_versions_and_missing_inventory_errors() {
        let directory = tempfile::tempdir().unwrap();
        let location = directory.path().canonicalize().unwrap();
        let fixture = serde_json::json!({"apps": [
            {"app":"dsh", "path":location, "version":"0.2.1-rc.2"},
            {"app":"cindy", "path":location, "version":" "}
        ], "errors":["native registration unavailable"]});
        let inventory = read_native_inventory(&fixture.to_string()).unwrap();
        assert_eq!(inventory.apps[0].version.as_deref(), Some("0.2.1-rc.2"));
        assert_eq!(inventory.apps[1].version, None);
        assert_eq!(inventory.errors, vec!["native registration unavailable"]);
        assert!(read_native_inventory("invalid JSON").is_err());
    }
}
