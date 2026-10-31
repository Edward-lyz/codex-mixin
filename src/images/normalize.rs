use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ColorType, DynamicImage, ImageEncoder, ImageReader, Rgb, RgbImage};
use serde_json::{Value, json};
use std::io::Cursor;
use std::ops::Range;
use std::sync::OnceLock;
use tokio::sync::Semaphore;

use crate::error::GatewayError;

const MAX_VISION_DIM: u32 = 1568;
const FALLBACK_VISION_DIM: u32 = 768;
const MAX_DECODE_PIXELS: u64 = 50_000_000;
const MAX_CONCURRENT_IMAGE_NORMALIZATIONS: usize = 2;
const TOOL_IMAGE_PLACEHOLDER: &str = "[tool image omitted from replay to preserve prompt cache]";

static IMAGE_NORMALIZATION_PERMITS: Semaphore =
    Semaphore::const_new(MAX_CONCURRENT_IMAGE_NORMALIZATIONS);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ImageNormalizationStats {
    pub normalized_images: usize,
    pub omitted_tool_images: usize,
    pub saved_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImageCompressionProfile {
    Primary,
    PayloadFallback,
}

pub(crate) async fn normalize_provider_images_blocking(
    mut value: Value,
) -> Result<(Value, ImageNormalizationStats), GatewayError> {
    run_image_work(move || {
        canonicalize_provider_json(&mut value);
        let stats =
            normalize_provider_images_with_profile(&mut value, ImageCompressionProfile::Primary)?;
        Ok((value, stats))
    })
    .await
}

pub(crate) async fn normalize_provider_images_for_fallback(
    mut value: Value,
) -> Result<(Value, ImageNormalizationStats), GatewayError> {
    run_image_work(move || {
        let stats = normalize_provider_images_with_profile(
            &mut value,
            ImageCompressionProfile::PayloadFallback,
        )?;
        Ok((value, stats))
    })
    .await
}

pub(crate) async fn normalize_anthropic_images_blocking(
    mut value: Value,
    profile: ImageCompressionProfile,
) -> Result<(Value, ImageNormalizationStats), GatewayError> {
    run_image_work(move || {
        let mut stats = ImageNormalizationStats::default();
        normalize_anthropic_images(&mut value, profile, &mut stats)?;
        Ok((value, stats))
    })
    .await
}

async fn run_image_work<T, F>(work: F) -> Result<T, GatewayError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, GatewayError> + Send + 'static,
{
    let permit = IMAGE_NORMALIZATION_PERMITS
        .acquire()
        .await
        .map_err(|error| GatewayError::Other(anyhow::Error::new(error)))?;

    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|error| GatewayError::Other(anyhow::Error::new(error)))?
}

fn normalize_provider_images_with_profile(
    body: &mut Value,
    profile: ImageCompressionProfile,
) -> Result<ImageNormalizationStats, GatewayError> {
    let mut stats = ImageNormalizationStats::default();
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return Ok(stats);
    };
    let fresh_outputs = trailing_tool_output_run(input);
    for (index, item) in input.iter_mut().enumerate() {
        normalize_input_item_images(item, fresh_outputs.contains(&index), profile, &mut stats)?;
    }
    Ok(stats)
}

/// Indices of the tool outputs the model has not seen yet: the last contiguous
/// run of tool outputs in the request, provided the model has not answered them.
///
/// Those results are new in this request, so inlining their images costs nothing
/// the provider had already cached. Every earlier tool output is settled history
/// that has to stay byte-identical, so its images become a stable marker. When
/// the model answers, this run stops being the tail and its images are replaced
/// on the next turn, which rewrites only what was previously the last message.
fn trailing_tool_output_run(input: &[Value]) -> Range<usize> {
    let Some(end) = input.iter().rposition(is_tool_output) else {
        return 0..0;
    };
    if input[end + 1..].iter().any(is_model_output) {
        return 0..0;
    }
    let mut start = end;
    while start > 0 && is_tool_output(&input[start - 1]) {
        start -= 1;
    }
    start..end + 1
}

fn is_tool_output(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call_output" | "custom_tool_call_output")
    )
}

/// Items the model produced. Everything a client contributes is either a
/// user-side message or a tool result, so treating the rest as model output errs
/// toward cache stability.
fn is_model_output(item: &Value) -> bool {
    if is_tool_output(item) {
        return false;
    }
    match item.get("type").and_then(Value::as_str) {
        Some("message") => !matches!(
            item.get("role").and_then(Value::as_str),
            Some("user" | "developer" | "system")
        ),
        _ => true,
    }
}

/// Sorts JSON object keys so provider-visible request bytes do not depend on the
/// insertion order of an incoming Responses payload.
///
/// `serde_json` already serializes objects in key order unless something in the
/// dependency graph enables its `preserve_order` feature, and Cargo features are
/// additive across the graph. Probing once keeps the guarantee without walking
/// every request body in the common build.
pub(super) fn canonicalize_provider_json(value: &mut Value) {
    if json_object_order_is_stable() {
        return;
    }
    sort_object_keys(value);
}

fn json_object_order_is_stable() -> bool {
    static STABLE: OnceLock<bool> = OnceLock::new();
    *STABLE.get_or_init(|| {
        let mut probe = serde_json::Map::new();
        probe.insert("b".to_owned(), Value::Bool(true));
        probe.insert("a".to_owned(), Value::Bool(false));
        serde_json::to_vec(&Value::Object(probe))
            .is_ok_and(|encoded| encoded.as_slice() == br#"{"a":false,"b":true}"#)
    })
}

fn sort_object_keys(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                sort_object_keys(item);
            }
        }
        Value::Object(object) => {
            let mut entries = std::mem::take(object).into_iter().collect::<Vec<_>>();
            for (_, value) in &mut entries {
                sort_object_keys(value);
            }
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            object.extend(entries);
        }
        _ => {}
    }
}

fn normalize_input_item_images(
    item: &mut Value,
    is_fresh_tool_output: bool,
    profile: ImageCompressionProfile,
    stats: &mut ImageNormalizationStats,
) -> Result<(), GatewayError> {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => {
            if let Some(parts) = item.get_mut("content").and_then(Value::as_array_mut) {
                for part in parts {
                    normalize_image_part(part, profile, stats)?;
                }
            }
        }
        Some("function_call_output" | "custom_tool_call_output") => {
            if !is_fresh_tool_output {
                replace_tool_output_images(item, stats);
                return Ok(());
            }
            // A fresh observation still has to fit the payload budget. Failing the
            // whole turn over one screenshot is worse than sending the marker, so
            // degrade instead of erroring.
            if let Err(error) = normalize_tool_output_images(item, profile, stats) {
                tracing::warn!(
                    error = %error,
                    "tool image could not be normalized, sending the cache-stable marker instead"
                );
                replace_tool_output_images(item, stats);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Compresses the images of a tool output, committing only if every image in it
/// normalizes, so a failure cannot leave a half-rewritten history item.
fn normalize_tool_output_images(
    item: &mut Value,
    profile: ImageCompressionProfile,
    stats: &mut ImageNormalizationStats,
) -> Result<(), GatewayError> {
    let Some(parts) = item.get("output").and_then(Value::as_array) else {
        return Ok(());
    };
    let mut normalized = parts.clone();
    let mut fresh = ImageNormalizationStats::default();
    for part in &mut normalized {
        normalize_image_part(part, profile, &mut fresh)?;
    }
    if fresh.normalized_images > 0 {
        item["output"] = Value::Array(normalized);
        stats.normalized_images += fresh.normalized_images;
        stats.saved_bytes += fresh.saved_bytes;
    }
    Ok(())
}

fn replace_tool_output_images(item: &mut Value, stats: &mut ImageNormalizationStats) {
    let Some(parts) = item.get_mut("output").and_then(Value::as_array_mut) else {
        return;
    };
    let mut removed = 0;
    let mut removed_bytes = 0;
    parts.retain(|part| {
        let embedded_image = (part.get("type").and_then(Value::as_str) == Some("input_image"))
            && part
                .get("image_url")
                .and_then(image_url_str)
                .is_some_and(|url| url.starts_with("data:image/"));
        if embedded_image {
            removed += 1;
            removed_bytes += part
                .get("image_url")
                .and_then(image_url_str)
                .map_or(0, str::len);
            false
        } else {
            true
        }
    });
    if removed == 0 {
        return;
    }
    stats.omitted_tool_images += removed;
    stats.saved_bytes += removed_bytes;
    // Always leave a marker. Dropping the image silently would let the model
    // read the surrounding text as the complete tool result.
    parts.push(json!({"type":"input_text","text":TOOL_IMAGE_PLACEHOLDER}));
}

fn normalize_image_part(
    part: &mut Value,
    profile: ImageCompressionProfile,
    stats: &mut ImageNormalizationStats,
) -> Result<(), GatewayError> {
    if part.get("type").and_then(Value::as_str) != Some("input_image") {
        return Ok(());
    }
    let Some(image_url) = part.get_mut("image_url") else {
        return Ok(());
    };
    let Some(original_url) = image_url_str(image_url).map(str::to_owned) else {
        return Ok(());
    };
    let Some((media_type, data)) = data_image_url_parts(&original_url) else {
        return Ok(());
    };
    let raw = match STANDARD.decode(data) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::warn!(%error, "input image is not valid base64; forwarding it unchanged");
            return Ok(());
        }
    };
    let Some(normalized) = normalize_image_bytes(media_type, &raw, profile)? else {
        return Ok(());
    };
    stats.normalized_images += 1;
    stats.saved_bytes += original_url.len().saturating_sub(normalized.len());
    match image_url {
        Value::String(url) => *url = normalized,
        Value::Object(object) => {
            object.insert("url".to_owned(), Value::String(normalized));
        }
        _ => {}
    }
    Ok(())
}

fn normalize_anthropic_images(
    value: &mut Value,
    profile: ImageCompressionProfile,
    stats: &mut ImageNormalizationStats,
) -> Result<(), GatewayError> {
    match value {
        Value::Array(items) => {
            for item in items {
                normalize_anthropic_images(item, profile, stats)?;
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("image")
                && let Some(source) = object.get_mut("source").and_then(Value::as_object_mut)
                && source.get("type").and_then(Value::as_str) == Some("base64")
            {
                let media_type = source
                    .get("media_type")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let data = source
                    .get("data")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let (Some(media_type), Some(data)) = (media_type, data) {
                    let raw = match STANDARD.decode(&data) {
                        Ok(raw) => raw,
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                "Anthropic image is not valid base64; forwarding it unchanged"
                            );
                            return Ok(());
                        }
                    };
                    if let Some(normalized) = normalize_image_bytes(&media_type, &raw, profile)?
                        && let Some((normalized_media_type, normalized_data)) =
                            data_image_url_parts(&normalized)
                    {
                        stats.normalized_images += 1;
                        stats.saved_bytes += data.len().saturating_sub(normalized_data.len());
                        source.insert(
                            "media_type".to_owned(),
                            Value::String(normalized_media_type.to_owned()),
                        );
                        source.insert("data".to_owned(), Value::String(normalized_data.to_owned()));
                    }
                }
                return Ok(());
            }
            for nested in object.values_mut() {
                normalize_anthropic_images(nested, profile, stats)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn image_url_str(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("url").and_then(Value::as_str))
}

fn data_image_url_parts(url: &str) -> Option<(&str, &str)> {
    let data_url = url.strip_prefix("data:")?;
    let (media_type, data) = data_url.split_once(";base64,")?;
    if media_type.starts_with("image/") {
        Some((media_type, data))
    } else {
        None
    }
}

fn normalize_image_bytes(
    media_type: &str,
    raw: &[u8],
    profile: ImageCompressionProfile,
) -> Result<Option<String>, GatewayError> {
    if raw.is_empty() {
        return Ok(None);
    }
    let Some((compressed, compressed_media_type)) = compress_for_vision(raw, media_type, profile)?
    else {
        return Ok(None);
    };
    if compressed.len() >= raw.len() {
        return Ok(None);
    }
    Ok(Some(format!(
        "data:{compressed_media_type};base64,{}",
        STANDARD.encode(compressed)
    )))
}

/// Guards against decompression bombs by refusing to decode images whose
/// declared dimensions blow past the pixel budget.
fn exceeds_decode_budget(width: u32, height: u32) -> bool {
    u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS
}

fn compress_for_vision<'a>(
    raw: &'a [u8],
    media_type: &'a str,
    profile: ImageCompressionProfile,
) -> Result<Option<(Vec<u8>, &'a str)>, GatewayError> {
    if !matches!(
        media_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) {
        return Ok(None);
    }
    let Some((format, width, height)) = inspect_image(raw) else {
        return Ok(None);
    };
    if exceeds_decode_budget(width, height) {
        tracing::warn!(
            width,
            height,
            "input image exceeds the safe decode pixel budget; forwarding unchanged"
        );
        return Ok(None);
    }
    if should_forward_gif_unchanged(media_type, profile, width, height) {
        return Ok(None);
    }

    let decoded = match ImageReader::with_format(Cursor::new(raw), format).decode() {
        Ok(decoded) => decoded,
        Err(error) => {
            tracing::warn!(%error, "input image could not be decoded; forwarding unchanged");
            return Ok(None);
        }
    };
    let (resized, target_width, target_height) = resize_for_vision(decoded, width, height, profile);
    encode_for_vision(resized, media_type, profile, target_width, target_height)
}

fn inspect_image(raw: &[u8]) -> Option<(image::ImageFormat, u32, u32)> {
    let reader = match ImageReader::new(Cursor::new(raw)).with_guessed_format() {
        Ok(reader) => reader,
        Err(error) => {
            tracing::warn!(%error, "input image format could not be detected; forwarding unchanged");
            return None;
        }
    };
    let format = reader.format()?;
    match reader.into_dimensions() {
        Ok((width, height)) => Some((format, width, height)),
        Err(error) => {
            tracing::warn!(%error, "input image dimensions could not be read; forwarding unchanged");
            None
        }
    }
}

fn should_forward_gif_unchanged(
    media_type: &str,
    profile: ImageCompressionProfile,
    width: u32,
    height: u32,
) -> bool {
    profile == ImageCompressionProfile::Primary
        && media_type == "image/gif"
        && width <= MAX_VISION_DIM
        && height <= MAX_VISION_DIM
}

fn resize_for_vision(
    decoded: DynamicImage,
    width: u32,
    height: u32,
    profile: ImageCompressionProfile,
) -> (DynamicImage, u32, u32) {
    let max_side = match profile {
        ImageCompressionProfile::Primary => MAX_VISION_DIM,
        ImageCompressionProfile::PayloadFallback => FALLBACK_VISION_DIM,
    };
    let (target_width, target_height) = if width > max_side || height > max_side {
        scaled_dims(width, height, max_side)
    } else {
        (width, height)
    };
    let resized = if (target_width, target_height) == (width, height) {
        decoded
    } else {
        decoded.resize_exact(
            target_width,
            target_height,
            image::imageops::FilterType::CatmullRom,
        )
    };
    (resized, target_width, target_height)
}

fn encode_for_vision(
    resized: DynamicImage,
    media_type: &str,
    profile: ImageCompressionProfile,
    target_width: u32,
    target_height: u32,
) -> Result<Option<(Vec<u8>, &str)>, GatewayError> {
    let mut out = Vec::new();
    let preserve_alpha = profile == ImageCompressionProfile::Primary
        && media_type == "image/png"
        && image_has_transparency(&resized);
    let output_media_type = if preserve_alpha {
        "image/png"
    } else {
        "image/jpeg"
    };
    if preserve_alpha {
        let rgba = resized.to_rgba8();
        if let Err(error) = PngEncoder::new(&mut out).write_image(
            &rgba,
            target_width,
            target_height,
            ColorType::Rgba8.into(),
        ) {
            tracing::warn!(%error, "input image could not be encoded; forwarding unchanged");
            return Ok(None);
        }
    } else {
        let quality = match profile {
            ImageCompressionProfile::Primary => 85,
            ImageCompressionProfile::PayloadFallback => 65,
        };
        if let Err(error) = JpegEncoder::new_with_quality(&mut out, quality)
            .encode_image(&DynamicImage::ImageRgb8(flatten_onto_white(&resized)))
        {
            tracing::warn!(%error, "input image could not be encoded; forwarding unchanged");
            return Ok(None);
        }
    }
    Ok(Some((out, output_media_type)))
}

fn image_has_transparency(image: &DynamicImage) -> bool {
    image.to_rgba8().pixels().any(|pixel| pixel[3] != u8::MAX)
}

fn flatten_onto_white(image: &DynamicImage) -> RgbImage {
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let pixel = rgba.get_pixel(x, y);
        let alpha = u16::from(pixel[3]);
        let blend =
            |channel: u8| ((u16::from(channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
        Rgb([blend(pixel[0]), blend(pixel[1]), blend(pixel[2])])
    })
}

fn scaled_dims(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    if width >= height {
        let scaled_height = (u64::from(height) * u64::from(max_side) / u64::from(width)).max(1);
        (max_side, scaled_height as u32)
    } else {
        let scaled_width = (u64::from(width) * u64::from(max_side) / u64::from(height)).max(1);
        (scaled_width as u32, max_side)
    }
}
