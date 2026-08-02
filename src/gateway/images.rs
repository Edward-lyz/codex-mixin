use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder, ImageReader};
use serde_json::{Value, json};
use std::io::Cursor;

use crate::error::GatewayError;

const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_VISION_DIM: u32 = 1568;
const MAX_DECODE_PIXELS: u64 = 50_000_000;
const TOOL_IMAGE_PLACEHOLDER: &str = "[tool image omitted from replay to preserve prompt cache]";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ImageNormalizationStats {
    pub normalized_images: usize,
    pub omitted_tool_images: usize,
    pub saved_bytes: usize,
}

pub(super) fn normalize_provider_images(
    body: &mut Value,
) -> Result<ImageNormalizationStats, GatewayError> {
    let mut stats = ImageNormalizationStats::default();
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return Ok(stats);
    };
    for item in input {
        normalize_input_item_images(item, &mut stats)?;
    }
    Ok(stats)
}

/// Recursively sorts JSON object keys so provider-visible request bytes do not
/// depend on the insertion order used by an incoming Responses payload.
pub(crate) fn canonicalize_provider_json(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                canonicalize_provider_json(item);
            }
        }
        Value::Object(object) => {
            let mut entries = std::mem::take(object).into_iter().collect::<Vec<_>>();
            for (_, value) in &mut entries {
                canonicalize_provider_json(value);
            }
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            object.extend(entries);
        }
        _ => {}
    }
}

fn normalize_input_item_images(
    item: &mut Value,
    stats: &mut ImageNormalizationStats,
) -> Result<(), GatewayError> {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => {
            if let Some(parts) = item.get_mut("content").and_then(Value::as_array_mut) {
                for part in parts {
                    normalize_image_part(part, stats)?;
                }
            }
        }
        Some("function_call_output" | "custom_tool_call_output") => {
            replace_tool_output_images(item, stats);
        }
        _ => {}
    }
    Ok(())
}

fn replace_tool_output_images(item: &mut Value, stats: &mut ImageNormalizationStats) {
    let Some(parts) = item.get_mut("output").and_then(Value::as_array_mut) else {
        return;
    };
    let mut removed = 0;
    parts.retain(|part| {
        let is_embedded_image = part.get("type").and_then(Value::as_str) == Some("input_image")
            && part
                .get("image_url")
                .and_then(image_url_str)
                .is_some_and(|url| url.starts_with("data:image/"));
        if is_embedded_image {
            removed += 1;
            false
        } else {
            true
        }
    });
    if removed == 0 {
        return;
    }
    stats.omitted_tool_images += removed;
    if parts.is_empty() {
        parts.push(json!({"type":"input_text","text":TOOL_IMAGE_PLACEHOLDER}));
    }
}

fn normalize_image_part(
    part: &mut Value,
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
    let raw = STANDARD.decode(data).map_err(|error| {
        GatewayError::BadRequest(format!("input_image data URL is not valid base64: {error}"))
    })?;
    let normalized = normalize_image_bytes(media_type, &raw)?;
    if normalized.url != original_url {
        stats.normalized_images += 1;
        stats.saved_bytes += original_url.len().saturating_sub(normalized.url.len());
        *image_url = match image_url {
            Value::String(_) => Value::String(normalized.url),
            Value::Object(object) => {
                object.insert("url".to_owned(), Value::String(normalized.url));
                return Ok(());
            }
            _ => return Ok(()),
        };
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

struct NormalizedImage {
    url: String,
}

fn normalize_image_bytes(media_type: &str, raw: &[u8]) -> Result<NormalizedImage, GatewayError> {
    if raw.is_empty() || raw.len() > MAX_IMAGE_BYTES {
        return Err(GatewayError::BadRequest(format!(
            "input_image must be between 1 byte and {} MB",
            MAX_IMAGE_BYTES / 1024 / 1024
        )));
    }
    let (raw, media_type) = compress_for_vision(raw, media_type)?;
    if raw.len() > MAX_IMAGE_BYTES {
        return Err(GatewayError::BadRequest(format!(
            "input_image remains larger than {} MB after compression",
            MAX_IMAGE_BYTES / 1024 / 1024
        )));
    }
    Ok(NormalizedImage {
        url: format!("data:{media_type};base64,{}", STANDARD.encode(raw)),
    })
}

fn compress_for_vision<'a>(
    raw: &'a [u8],
    media_type: &'a str,
) -> Result<(Vec<u8>, &'a str), GatewayError> {
    if !matches!(
        media_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) {
        return Ok((raw.to_vec(), media_type));
    }
    let reader = ImageReader::new(Cursor::new(raw)).with_guessed_format()?;
    let Some(format) = reader.format() else {
        return Ok((raw.to_vec(), media_type));
    };
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| GatewayError::BadRequest(format!("decode input_image: {error}")))?;
    if u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS {
        return Ok((raw.to_vec(), media_type));
    }
    if width <= MAX_VISION_DIM && height <= MAX_VISION_DIM {
        return Ok((raw.to_vec(), media_type));
    }

    let decoded = ImageReader::with_format(Cursor::new(raw), format)
        .decode()
        .map_err(|error| GatewayError::BadRequest(format!("decode input_image: {error}")))?;
    let (target_width, target_height) = scaled_dims(width, height, MAX_VISION_DIM);
    let resized = decoded.resize_exact(
        target_width,
        target_height,
        image::imageops::FilterType::CatmullRom,
    );
    let rgba = resized.to_rgba8();
    let mut out = Vec::new();
    if matches!(media_type, "image/png" | "image/gif") {
        PngEncoder::new(&mut out)
            .write_image(&rgba, target_width, target_height, ColorType::Rgba8.into())
            .map_err(|error| GatewayError::BadRequest(format!("encode input_image: {error}")))?;
        Ok((out, "image/png"))
    } else {
        JpegEncoder::new_with_quality(&mut out, 85)
            .encode_image(&resized)
            .map_err(|error| GatewayError::BadRequest(format!("encode input_image: {error}")))?;
        Ok((out, "image/jpeg"))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn png_data_url(width: u32, height: u32) -> String {
        let img = ImageBuffer::from_fn(width, height, |x, y| {
            Rgba([x as u8, y as u8, (x ^ y) as u8, 255])
        });
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&img, width, height, ColorType::Rgba8.into())
            .unwrap();
        format!("data:image/png;base64,{}", STANDARD.encode(bytes))
    }

    #[test]
    fn keeps_small_user_images_byte_stable() {
        let url = png_data_url(16, 16);
        let mut body = json!({"input":[{"type":"message","role":"user","content":[{"type":"input_image","image_url":url}]}]});

        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats, ImageNormalizationStats::default());
        assert_eq!(body["input"][0]["content"][0]["image_url"], url);
    }

    #[test]
    fn canonicalizes_nested_object_key_order_without_reordering_arrays() {
        let mut value = json!({
            "z": {"b": 2, "a": 1},
            "a": [{"d": 4, "c": 3}, "tail"]
        });

        canonicalize_provider_json(&mut value);

        assert_eq!(
            value.to_string(),
            r#"{"a":[{"c":3,"d":4},"tail"],"z":{"a":1,"b":2}}"#
        );
    }

    #[test]
    fn downscales_oversized_user_images_deterministically() {
        let url = png_data_url(3000, 1500);
        let mut first = json!({"input":[{"type":"message","role":"user","content":[{"type":"input_image","image_url":url}]}]});
        let mut second = first.clone();

        let first_stats = normalize_provider_images(&mut first).unwrap();
        let second_stats = normalize_provider_images(&mut second).unwrap();

        assert_eq!(first, second);
        assert_eq!(first_stats.normalized_images, 1);
        assert_eq!(second_stats.normalized_images, 1);
        let normalized = first["input"][0]["content"][0]["image_url"]
            .as_str()
            .unwrap();
        let (_, data) = data_image_url_parts(normalized).unwrap();
        let decoded = STANDARD.decode(data).unwrap();
        let reader = ImageReader::new(Cursor::new(decoded))
            .with_guessed_format()
            .unwrap();
        let (width, height) = reader.into_dimensions().unwrap();
        assert_eq!((width, height), (MAX_VISION_DIM, 784));
    }

    #[test]
    fn replaces_tool_output_images_with_stable_placeholders() {
        let mut body = json!({
            "input": [{
                "type": "function_call_output",
                "call_id": "call_1",
                "output": [{"type":"input_image","image_url":"data:image/png;base64,AAAA"}]
            }]
        });

        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats.omitted_tool_images, 1);
        assert_eq!(
            body["input"][0]["output"],
            json!([{"type":"input_text","text":TOOL_IMAGE_PLACEHOLDER}])
        );
    }

    #[test]
    fn preserves_tool_output_text_when_omitting_images() {
        let mut body = json!({
            "input": [{
                "type": "function_call_output",
                "call_id": "call_1",
                "output": [
                    {"type":"input_text","text":"screenshot captured"},
                    {"type":"input_image","image_url":"data:image/png;base64,AAAA"}
                ]
            }]
        });

        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats.omitted_tool_images, 1);
        assert_eq!(
            body["input"][0]["output"],
            json!([{"type":"input_text","text":"screenshot captured"}])
        );
    }
}
