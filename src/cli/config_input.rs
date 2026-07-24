pub(super) fn trim_required(label: &str, value: String) -> anyhow::Result<String> {
    let trimmed = value.trim().to_owned();
    if trimmed.is_empty() {
        anyhow::bail!("{label} cannot be empty");
    }
    Ok(trimmed)
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
