//! Shares the CLI workspace version with the desktop app and, on Windows,
//! embeds the app icon and version resource into the executable.

fn main() {
    println!("cargo:rerun-if-changed=locales");
    println!("cargo:rerun-if-changed=themes");
    println!("cargo:rerun-if-changed=assets");
    println!("cargo:rerun-if-changed=../Cargo.toml");
    let version = workspace_version().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    println!("cargo:rustc-env=CODEX_MIXIN_VERSION={version}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_resources(&version);
    }
}

/// Reads `[workspace.package] version` from the CLI's Cargo.toml, so the app
/// and the CLI it bundles always report the same release.
fn workspace_version() -> Option<String> {
    let manifest = std::fs::read_to_string("../Cargo.toml").ok()?;
    let section = manifest.split("[workspace.package]").nth(1)?;
    section
        .lines()
        .take_while(|line| !line.starts_with('['))
        .find_map(|line| line.trim().strip_prefix("version"))
        .and_then(|value| value.split('"').nth(1))
        .map(str::to_owned)
}

#[cfg(windows)]
fn embed_windows_resources(version: &str) {
    println!("cargo:rerun-if-changed=windows/CodexMixin.ico");
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("windows/CodexMixin.ico")
        .set("ProductName", "Codex Mixin")
        .set("FileDescription", "Codex Mixin")
        .set("ProductVersion", version)
        .set("FileVersion", version);
    resource.compile().expect("embed the Windows resources");
}

/// Windows builds run on Windows hosts, where the resource compiler lives.
#[cfg(not(windows))]
fn embed_windows_resources(_: &str) {}
