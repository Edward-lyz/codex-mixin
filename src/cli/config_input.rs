use std::io::{self, Write};

pub(super) fn prompt_required(label: &str) -> anyhow::Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    trim_required(label, value)
}

pub(super) fn trim_required(label: &str, value: String) -> anyhow::Result<String> {
    let trimmed = value.trim().to_owned();
    if trimmed.is_empty() {
        anyhow::bail!("{label} cannot be empty");
    }
    Ok(trimmed)
}

pub(super) fn first_env_value(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

pub(super) fn normalize_base_url(value: String) -> anyhow::Result<String> {
    let mut trimmed = trim_required("base URL", value)?;
    while trimmed.ends_with('/') {
        trimmed.pop();
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        anyhow::bail!("base URL must start with http:// or https://");
    }
    Ok(trimmed)
}
