use super::*;

struct Sandbox {
    home: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn at(&self) -> Locations {
        let agy = self.home.path().join(AGY_FILE);
        Locations {
            agy: agy.is_file().then_some(agy),
            kiro: self.home.path().join(KIRO_FILE),
            ignore: self.home.path().join("oauth/discovery-ignored.json"),
        }
    }

    fn write(&self, relative: &str, body: &str) {
        let path = self.home.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
}

const KIRO_FILE: &str = ".aws/sso/cache/kiro-auth-token.json";
const AGY_FILE: &str = ".gemini/antigravity-cli/antigravity-oauth-token";

fn agy_token(refresh: &str) -> String {
    format!(
        r#"{{"auth_method":"consumer","token":{{"access_token":"a","refresh_token":"{refresh}","expiry":"2020-01-01T00:00:00Z"}}}}"#
    )
}

#[test]
fn nothing_is_discovered_without_logins() {
    let sandbox = Sandbox::new();
    assert!(discover_at(&sandbox.at(), &[]).unwrap().is_empty());
}

#[test]
fn refreshable_logins_are_discovered_as_preset_providers() {
    let sandbox = Sandbox::new();
    sandbox.write(AGY_FILE, &agy_token("r1"));
    sandbox.write(KIRO_FILE, r#"{"refreshToken":"k","profileArn":"arn:one"}"#);
    let found = discover_at(&sandbox.at(), &[]).unwrap();
    let ids: Vec<&str> = found.iter().map(|provider| provider.id.as_str()).collect();
    assert_eq!(ids, ["agy", "kiro"]);
    assert!(found.iter().all(|provider| {
        provider.discovered_credential.is_some()
            && provider.auth.api_key.is_empty()
            && provider.models_refresh_error.as_deref() == Some(PENDING_REFRESH)
    }));
}

#[test]
fn broken_expired_and_business_logins_are_not_discovered() {
    let sandbox = Sandbox::new();
    sandbox.write(KIRO_FILE, "{not json");
    sandbox.write(
        AGY_FILE,
        r#"{"auth_method":"gcp","token":{"access_token":"a","refresh_token":"r"}}"#,
    );
    assert!(discover_at(&sandbox.at(), &[]).unwrap().is_empty());
    sandbox.write(
        KIRO_FILE,
        r#"{"accessToken":"x","expiresAt":"2020-01-01T00:00:00Z"}"#,
    );
    assert!(discover_at(&sandbox.at(), &[]).unwrap().is_empty());
}

#[test]
fn configured_providers_of_the_same_preset_win_even_when_disabled() {
    let sandbox = Sandbox::new();
    sandbox.write(KIRO_FILE, r#"{"refreshToken":"k"}"#);
    let mut manual = ProviderPreset::Kiro.create("my-kiro", "");
    manual.enabled = false;
    assert!(discover_at(&sandbox.at(), &[manual]).unwrap().is_empty());
    let clash = ProviderPreset::Custom.create("kiro", "key");
    assert!(
        discover_at(&sandbox.at(), &[clash])
            .unwrap_err()
            .to_string()
            .contains("already used")
    );
}

#[test]
fn ignored_presets_do_not_come_back_until_unignored() {
    let sandbox = Sandbox::new();
    sandbox.write(KIRO_FILE, r#"{"refreshToken":"k"}"#);
    set_ignored_at(&sandbox.at(), "kiro", true).unwrap();
    assert!(discover_at(&sandbox.at(), &[]).unwrap().is_empty());
    set_ignored_at(&sandbox.at(), "kiro", false).unwrap();
    assert_eq!(discover_at(&sandbox.at(), &[]).unwrap().len(), 1);
}

#[test]
fn account_switch_resets_the_model_directory_and_keeps_the_provider() {
    let sandbox = Sandbox::new();
    sandbox.write(KIRO_FILE, r#"{"refreshToken":"k","profileArn":"arn:one"}"#);
    let mut providers = Vec::new();
    assert!(reconcile_at(&sandbox.at(), &mut providers).unwrap());
    providers[0]
        .cached_models
        .push(crate::provider::ProviderModel {
            id: "m".to_owned(),
            ..Default::default()
        });
    providers[0].selected_models.push("m".to_owned());
    providers[0].models_refresh_error = None;
    assert!(!reconcile_at(&sandbox.at(), &mut providers).unwrap());
    sandbox.write(KIRO_FILE, r#"{"refreshToken":"k2","profileArn":"arn:two"}"#);
    assert!(reconcile_at(&sandbox.at(), &mut providers).unwrap());
    assert!(providers[0].cached_models.is_empty());
    assert!(providers[0].selected_models.is_empty());
    assert_eq!(
        providers[0].models_refresh_error.as_deref(),
        Some(PENDING_REFRESH)
    );
}

#[test]
fn rotating_kiro_refresh_token_keeps_the_same_account() {
    let sandbox = Sandbox::new();
    sandbox.write(
        KIRO_FILE,
        r#"{"refreshToken":"old","profileArn":"arn:one"}"#,
    );
    let first = inspect(ProviderPreset::Kiro, &sandbox.at()).unwrap();
    sandbox.write(
        KIRO_FILE,
        r#"{"refreshToken":"new","profileArn":"arn:one"}"#,
    );
    assert_eq!(inspect(ProviderPreset::Kiro, &sandbox.at()).unwrap(), first);
}

#[test]
fn runtime_view_drops_accounts_whose_login_vanished() {
    let sandbox = Sandbox::new();
    sandbox.write(KIRO_FILE, r#"{"refreshToken":"k"}"#);
    let mut stored = Vec::new();
    reconcile_at(&sandbox.at(), &mut stored).unwrap();
    assert_eq!(
        runtime_providers_at(&sandbox.at(), stored.clone())
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_file(sandbox.home.path().join(KIRO_FILE)).unwrap();
    assert!(
        runtime_providers_at(&sandbox.at(), stored)
            .unwrap()
            .is_empty()
    );
}
