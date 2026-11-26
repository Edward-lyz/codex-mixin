//! Unified registry of framework agent integrations.
//!
//! Each framework agent declares its display name, integration driver kind,
//! launch/startup requirements, mutable fields, and disconnect key-revocation
//! semantics in one table rather than scattering `match id` checks across
//! status, install, remove, startup, and CLI orchestration.

use std::path::Path;

use crate::clients::IntegrationStatus;

use super::{FrameworkModel, alma, aside, cindy, hanako, zed};

#[derive(Clone, Copy)]
pub struct NativeAppDriver {
    pub status: fn(&Path) -> anyhow::Result<IntegrationStatus>,
    pub install: fn(&Path, &str, &str, &[FrameworkModel]) -> anyhow::Result<()>,
    pub remove: fn(&Path) -> anyhow::Result<()>,
}

#[derive(Clone, Copy)]
pub enum FrameworkKind {
    /// Declarative schema patches applied and journaled by `engine`.
    Schema,
    /// Vendor-native model selection (`cursor`, `copilot`, `devin`) without a custom gateway URL.
    NativeVendorModels,
    /// Dedicated native desktop/editor app integration driver.
    NativeApp(NativeAppDriver),
}

pub struct FrameworkSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: FrameworkKind,
    pub free_fields: &'static [&'static str],
    pub launch_required: bool,
    pub client_auth_launch_required: bool,
    pub revoke_key_before_remove: bool,
}

impl FrameworkSpec {
    const fn schema(id: &'static str, name: &'static str) -> Self {
        Self {
            id,
            name,
            kind: FrameworkKind::Schema,
            free_fields: &[],
            launch_required: false,
            client_auth_launch_required: false,
            revoke_key_before_remove: false,
        }
    }

    const fn schema_launch(id: &'static str, name: &'static str) -> Self {
        Self {
            launch_required: true,
            ..Self::schema(id, name)
        }
    }

    const fn schema_client_auth_launch(id: &'static str, name: &'static str) -> Self {
        Self {
            client_auth_launch_required: true,
            ..Self::schema(id, name)
        }
    }

    const fn native_models(id: &'static str, name: &'static str) -> Self {
        Self {
            kind: FrameworkKind::NativeVendorModels,
            ..Self::schema(id, name)
        }
    }

    const fn native_app(
        id: &'static str,
        name: &'static str,
        driver: NativeAppDriver,
        revoke_key_before_remove: bool,
    ) -> Self {
        Self {
            kind: FrameworkKind::NativeApp(driver),
            revoke_key_before_remove,
            ..Self::schema(id, name)
        }
    }

    pub const fn requires_launch(&self) -> bool {
        self.launch_required
            || (cfg!(feature = "client-auth") && self.client_auth_launch_required)
    }

    pub const fn is_native_models(&self) -> bool {
        matches!(self.kind, FrameworkKind::NativeVendorModels)
    }

    pub const fn native_app_driver(&self) -> Option<NativeAppDriver> {
        match self.kind {
            FrameworkKind::NativeApp(driver) => Some(driver),
            _ => None,
        }
    }
}

fn install_cindy(
    manifest: &Path,
    base: &str,
    key: &str,
    _models: &[FrameworkModel],
) -> anyhow::Result<()> {
    cindy::install(manifest, base, key)
}

const ZED_DRIVER: NativeAppDriver = NativeAppDriver {
    status: zed::status,
    install: zed::install,
    remove: zed::remove,
};

const ASIDE_DRIVER: NativeAppDriver = NativeAppDriver {
    status: aside::status,
    install: aside::install,
    remove: aside::remove,
};

const CINDY_DRIVER: NativeAppDriver = NativeAppDriver {
    status: cindy::status,
    install: install_cindy,
    remove: cindy::remove,
};

const ALMA_DRIVER: NativeAppDriver = NativeAppDriver {
    status: alma::status,
    install: alma::install,
    remove: alma::remove,
};

const HANAKO_DRIVER: NativeAppDriver = NativeAppDriver {
    status: hanako::status,
    install: hanako::install,
    remove: hanako::remove,
};

pub const FRAMEWORKS: &[FrameworkSpec] = &[
    FrameworkSpec::schema("gemini", "Gemini CLI"),
    FrameworkSpec {
        // agy rewrites `model` on `/model`; launches pass the model from the launch journal.
        free_fields: &["model"],
        ..FrameworkSpec::schema_launch("agy", "agy")
    },
    FrameworkSpec::schema("openchamber", "OpenChamber"),
    FrameworkSpec::schema("mimocode", "MiMo Code"),
    FrameworkSpec::native_app("aside", "Aside", ASIDE_DRIVER, false),
    FrameworkSpec::schema("omo", "OmO"),
    FrameworkSpec::schema("goose", "Goose"),
    FrameworkSpec::native_models("cursor", "Cursor CLI"),
    FrameworkSpec::schema_launch("cursor-local", "Cursor"),
    FrameworkSpec::native_app("zed", "Zed", ZED_DRIVER, false),
    FrameworkSpec::schema("vscode", "VS Code"),
    FrameworkSpec::schema("vscode-insiders", "VS Code Insiders"),
    FrameworkSpec::schema("vscodium", "VSCodium"),
    FrameworkSpec::schema("copilot-jetbrains", "Copilot for JetBrains"),
    FrameworkSpec::schema("air", "Air"),
    FrameworkSpec::native_models("copilot", "Copilot CLI"),
    FrameworkSpec::schema("crush", "Crush"),
    FrameworkSpec::schema("reasonix", "Reasonix"),
    FrameworkSpec::schema_launch("commandcode", "Command Code"),
    FrameworkSpec::schema_client_auth_launch("fx", "fx"),
    FrameworkSpec::schema("omp", "Oh My Pi"),
    FrameworkSpec::native_models("devin", "Devin"),
    FrameworkSpec::schema("hermes", "Hermes Agent"),
    FrameworkSpec::schema("morph", "Mister Morph"),
    FrameworkSpec::schema("kimi", "Kimi Code"),
    FrameworkSpec::schema("qwen", "Qwen Code"),
    FrameworkSpec::schema_launch("muse", "Muse"),
    FrameworkSpec::schema_client_auth_launch("empryo", "Empryo"),
    FrameworkSpec::schema_client_auth_launch("ante", "Ante"),
    FrameworkSpec::schema("minimax-code", "MiniMax Code"),
    FrameworkSpec::schema("droid", "Droid"),
    FrameworkSpec::schema("cline", "Cline"),
    FrameworkSpec::schema("qoder", "Qoder"),
    FrameworkSpec::schema("qoder-cn", "Qoder CN"),
    FrameworkSpec::schema("grok", "Grok Build"),
    FrameworkSpec::schema("zcode", "ZCode"),
    FrameworkSpec::schema("workbuddy", "WorkBuddy"),
    FrameworkSpec::schema("codebuddy", "CodeBuddy"),
    FrameworkSpec::schema("pencil", "Pencil"),
    FrameworkSpec::schema("t3code", "T3 Code"),
    FrameworkSpec::native_app("hanako", "Hanako", HANAKO_DRIVER, true),
    FrameworkSpec::schema("atomcode", "AtomCode"),
    FrameworkSpec::native_app("alma", "Alma", ALMA_DRIVER, false),
    FrameworkSpec::native_app("cindy", "Cindy", CINDY_DRIVER, true),
];

pub fn spec(id: &str) -> anyhow::Result<&'static FrameworkSpec> {
    FRAMEWORKS
        .iter()
        .find(|spec| spec.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown framework: {id}"))
}
