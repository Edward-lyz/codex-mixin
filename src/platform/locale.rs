use anyhow::Context;

/// Preferred OS display language, normalized for Claude Desktop profiles.
pub fn system_locale() -> anyhow::Result<String> {
    #[cfg(target_os = "macos")]
    let preferred = {
        let output = std::process::Command::new("/usr/bin/defaults")
            .args(["read", "-g", "AppleLanguages"])
            .output()
            .context("read macOS preferred languages")?;
        anyhow::ensure!(
            output.status.success(),
            "read macOS preferred languages: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let languages =
            String::from_utf8(output.stdout).context("decode macOS preferred languages")?;
        languages
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && *line != "(" && *line != ")")
            .context("macOS preferred language list is empty")?
            .trim_end_matches(',')
            .trim_matches('"')
            .to_owned()
    };
    #[cfg(windows)]
    let preferred = {
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[System.Globalization.CultureInfo]::CurrentUICulture.Name",
            ])
            .output()
            .context("read Windows display language")?;
        anyhow::ensure!(
            output.status.success(),
            "read Windows display language: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        String::from_utf8(output.stdout).context("decode Windows display language")?
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let preferred = ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .context("system display language is not configured")?;
    normalize_locale(&preferred)
}

fn normalize_locale(preferred: &str) -> anyhow::Result<String> {
    let preferred = preferred
        .trim()
        .split([':', '.', '@'])
        .next()
        .unwrap_or_default()
        .replace('_', "-");
    anyhow::ensure!(
        !preferred.is_empty()
            && preferred
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid system display language"
    );
    let lower = preferred.to_ascii_lowercase();
    let parts = lower.split('-').collect::<Vec<_>>();
    let language = parts[0];
    let locale = match language {
        "zh" if parts
            .iter()
            .any(|part| matches!(*part, "hant" | "tw" | "hk" | "mo")) =>
        {
            "zh-Hant"
        }
        "zh" => "zh-Hans",
        "en" | "c" | "posix" => "en-US",
        "de" => "de-DE",
        "fr" => "fr-FR",
        "hi" => "hi-IN",
        "id" => "id-ID",
        "it" => "it-IT",
        "ja" => "ja-JP",
        "ko" => "ko-KR",
        "pt" => "pt-BR",
        "es" if parts
            .iter()
            .any(|part| matches!(*part, "419" | "mx" | "ar" | "cl" | "co" | "pe")) =>
        {
            "es-419"
        }
        "es" => "es-ES",
        _ => return Ok(preferred),
    };
    Ok(locale.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_os_language_formats() {
        for (preferred, locale) in [
            ("zh-Hans-CN", "zh-Hans"),
            ("zh_CN.UTF-8", "zh-Hans"),
            ("zh-Hant-HK", "zh-Hant"),
            ("zh_TW.UTF-8", "zh-Hant"),
            ("ja_JP.UTF-8", "ja-JP"),
            ("de-CH", "de-DE"),
            ("es-MX", "es-419"),
            ("C.UTF-8", "en-US"),
            ("fr_FR:en_US", "fr-FR"),
            ("nl-NL", "nl-NL"),
        ] {
            assert_eq!(normalize_locale(preferred).unwrap(), locale);
        }
        assert!(normalize_locale("").is_err());
        assert!(normalize_locale("../../locale").is_err());
    }
}
