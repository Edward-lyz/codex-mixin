use std::fs;
use std::path::Path;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;

pub(super) fn is_logged_in(isolated_home: &Path) -> bool {
    // DUCX writes the signed-in identity under HOME/.comate/login-user/<username>.
    let login_dir = isolated_home.join(".comate/login-user");
    fs::read_dir(&login_dir)
        .map(|entries| {
            let files = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .take(2)
                .collect::<Vec<_>>();
            files.len() == 1
                && fs::read_to_string(files[0].path())
                    .is_ok_and(|token| is_valid_login_token(token.trim()))
        })
        .unwrap_or(false)
}

fn is_valid_login_token(token: &str) -> bool {
    let token = token
        .strip_prefix("Bearer-")
        .or_else(|| token.strip_prefix("Bearer "))
        .unwrap_or(token);
    let mut segments = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return false;
    };
    if signature.is_empty() {
        return false;
    }
    let Ok(header) = URL_SAFE_NO_PAD.decode(header) else {
        return false;
    };
    let Ok(payload) = URL_SAFE_NO_PAD.decode(payload) else {
        return false;
    };
    let Ok(header) = serde_json::from_slice::<Value>(&header) else {
        return false;
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&payload) else {
        return false;
    };
    if header.get("alg").and_then(Value::as_str).is_none()
        || payload.get("sub").and_then(Value::as_str).is_none()
        || payload.get("iat").and_then(Value::as_i64).is_none()
    {
        return false;
    }
    match payload.get("exp").and_then(Value::as_i64) {
        None => true,
        Some(expires_at) => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .is_ok_and(|now| expires_at > now.as_secs() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_logged_in, is_valid_login_token};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use std::fs;

    #[test]
    fn login_state_is_scoped_to_isolated_home() {
        let root = tempfile::tempdir().unwrap();
        assert!(!is_logged_in(root.path()));
        let login_dir = root.path().join(".comate/login-user");
        fs::create_dir_all(&login_dir).unwrap();
        fs::write(login_dir.join("liyanzhen01"), valid_token()).unwrap();
        assert!(is_logged_in(root.path()));
        fs::write(login_dir.join("another-user"), b"{}").unwrap();
        assert!(!is_logged_in(root.path()));
    }

    #[test]
    fn rejects_stale_login_markers() {
        assert!(!is_valid_login_token(""));
        assert!(!is_valid_login_token("{}"));
        assert!(!is_valid_login_token("header.payload.signature"));
    }

    #[test]
    fn accepts_the_bearer_jwt_format_written_by_ducx() {
        assert!(is_valid_login_token(&format!("Bearer-{}", valid_token())));
        assert!(is_valid_login_token(&format!("Bearer {}", valid_token())));
    }

    fn valid_token() -> String {
        let encode = |value: &str| URL_SAFE_NO_PAD.encode(value.as_bytes());
        format!(
            "{}.{}.signature",
            encode(r#"{"alg":"RS256"}"#),
            encode(r#"{"sub":"user","iat":1786600000}"#)
        )
    }
}
