//! Desktop sign-in preferences. Gateway startup is owned by the CLI.

#[cfg(windows)]
const SHELL_SETTINGS: &str = r"Software\CodexMixin";
#[cfg(windows)]
const LIGHTWEIGHT: &str = "LightweightMode";

#[cfg(windows)]
pub fn skip_launch(background: bool) -> bool {
    use windows_registry::CURRENT_USER;

    let lightweight = CURRENT_USER
        .open(SHELL_SETTINGS)
        .and_then(|key| key.get_u32(LIGHTWEIGHT))
        .is_ok_and(|value| value != 0);
    if lightweight
        && !background
        && let Err(error) = CURRENT_USER
            .options()
            .write()
            .open(SHELL_SETTINGS)
            .and_then(|key| key.remove_value(LIGHTWEIGHT))
    {
        eprintln!("cannot clear desktop lightweight preference: {error}");
    }
    background && lightweight
}

#[cfg(not(windows))]
pub fn skip_launch(_: bool) -> bool {
    false
}

pub fn enter_lightweight() -> anyhow::Result<()> {
    #[cfg(windows)]
    windows_registry::CURRENT_USER
        .create(SHELL_SETTINGS)?
        .set_u32(LIGHTWEIGHT, 1)?;
    Ok(())
}
