//! Embedded assets: GPUI Kit's Lucide icons plus the product's provider,
//! agent, tray and card artwork.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[exclude = "*.md"]
struct ProductAssets;

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = ProductAssets::get(path) {
            return Ok(Some(file.data));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut names: Vec<SharedString> = ProductAssets::iter()
            .filter(|name| name.starts_with(path))
            .map(|name| SharedString::from(name.to_string()))
            .collect();
        names.extend(gpui_kit::assets::AllAssets.list(path)?);
        Ok(names)
    }
}

/// Raw bytes of an embedded product asset, for consumers outside GPUI's
/// asset pipeline such as the tray icon.
pub fn product_bytes(path: &str) -> Option<Cow<'static, [u8]>> {
    ProductAssets::get(path).map(|file| file.data)
}
