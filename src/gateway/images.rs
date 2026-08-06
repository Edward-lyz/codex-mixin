use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder, ImageReader};
use serde_json::{Value, json};
use std::io::Cursor;
use std::ops::Range;
use std::sync::OnceLock;
use tokio::sync::Semaphore;

use crate::error::GatewayError;

const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_VISION_DIM: u32 = 1568;
const MAX_DECODE_PIXELS: u64 = 50_000_000;
const MAX_CONCURRENT_IMAGE_NORMALIZATIONS: usize = 2;
const TOOL_IMAGE_PLACEHOLDER: &str = "[tool image omitted from replay to preserve prompt cache]";

static IMAGE_NORMALIZATION_PERMITS: Semaphore =
    Semaphore::const_new(MAX_CONCURRENT_IMAGE_NORMALIZATIONS);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ImageNormalizationStats {
    pub normalized_images: usize,
    pub omitted_tool_images: usize,
    pub saved_bytes: usize,
}

pub(super) async fn normalize_provider_images_blocking(
    mut value: Value,
) -> Result<(Value, ImageNormalizationStats), GatewayError> {
    run_image_work(move || {
        let stats = normalize_provider_images(&mut value)?;
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

pub(super) fn normalize_provider_images(
    body: &mut Value,
) -> Result<ImageNormalizationStats, GatewayError> {
    let mut stats = ImageNormalizationStats::default();
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return Ok(stats);
    };
    let fresh_outputs = trailing_tool_output_run(input);
    for (index, item) in input.iter_mut().enumerate() {
        normalize_input_item_images(item, fresh_outputs.contains(&index), &mut stats)?;
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
        Value::Object(probe).to_string() == r#"{"a":false,"b":true}"#
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
            if !is_fresh_tool_output {
                replace_tool_output_images(item, stats);
                return Ok(());
            }
            // A fresh observation still has to fit the payload budget. Failing the
            // whole turn over one screenshot is worse than sending the marker, so
            // degrade instead of erroring.
            if let Err(error) = normalize_tool_output_images(item, stats) {
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
    stats: &mut ImageNormalizationStats,
) -> Result<(), GatewayError> {
    let Some(parts) = item.get("output").and_then(Value::as_array) else {
        return Ok(());
    };
    let mut normalized = parts.clone();
    let mut fresh = ImageNormalizationStats::default();
    for part in &mut normalized {
        normalize_image_part(part, &mut fresh)?;
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
    if normalized == original_url {
        return Ok(());
    }
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

fn normalize_image_bytes(media_type: &str, raw: &[u8]) -> Result<String, GatewayError> {
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
    Ok(format!("data:{media_type};base64,{}", STANDARD.encode(raw)))
}

/// Encoding used once an image has to be re-encoded for the vision budget. PNG
/// keeps screenshot alpha intact; everything else is cheaper as JPEG.
fn vision_output_media_type(media_type: &str) -> &'static str {
    match media_type {
        "image/png" | "image/gif" => "image/png",
        _ => "image/jpeg",
    }
}

/// Guards against decompression bombs by refusing to decode images whose
/// declared dimensions blow past the pixel budget.
fn exceeds_decode_budget(width: u32, height: u32) -> bool {
    u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS
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
    if exceeds_decode_budget(width, height) {
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
    let mut out = Vec::new();
    let output_media_type = vision_output_media_type(media_type);
    if output_media_type == "image/png" {
        let rgba = resized.to_rgba8();
        PngEncoder::new(&mut out)
            .write_image(&rgba, target_width, target_height, ColorType::Rgba8.into())
            .map_err(|error| GatewayError::BadRequest(format!("encode input_image: {error}")))?;
    } else {
        JpegEncoder::new_with_quality(&mut out, 85)
            .encode_image(&resized)
            .map_err(|error| GatewayError::BadRequest(format!("encode input_image: {error}")))?;
    }
    Ok((out, output_media_type))
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
    use image::{DynamicImage, ImageBuffer, Rgb, Rgba};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

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

    fn jpeg_data_url(width: u32, height: u32) -> String {
        let img =
            ImageBuffer::from_fn(width, height, |x, y| Rgb([x as u8, y as u8, (x ^ y) as u8]));
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 90)
            .encode_image(&DynamicImage::ImageRgb8(img))
            .unwrap();
        format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes))
    }

    fn user_image_body(image_url: Value) -> Value {
        json!({"input":[{
            "type":"message",
            "role":"user",
            "content":[{"type":"input_image","image_url":image_url}]
        }]})
    }

    /// History with one replayed tool output and one the model has not seen yet.
    fn tool_history(replayed_output: Value, fresh_output: Value) -> Value {
        json!({"input":[
            {"type":"function_call","call_id":"call_old","name":"view_image","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_old","output":replayed_output},
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"looked"}]},
            {"type":"function_call","call_id":"call_new","name":"view_image","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_new","output":fresh_output}
        ]})
    }

    fn normalized_dimensions(image_url: &str) -> (String, u32, u32) {
        let (media_type, data) = data_image_url_parts(image_url).unwrap();
        let decoded = STANDARD.decode(data).unwrap();
        let (width, height) = ImageReader::new(Cursor::new(decoded))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        (media_type.to_owned(), width, height)
    }

    #[test]
    fn keeps_small_user_images_byte_stable() {
        let url = png_data_url(16, 16);
        let mut body = user_image_body(Value::String(url.clone()));

        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats, ImageNormalizationStats::default());
        assert_eq!(body["input"][0]["content"][0]["image_url"], url);
    }

    #[test]
    fn serde_json_orders_object_keys_so_request_bytes_stay_stable() {
        // A dependency enabling `serde_json/preserve_order` would make provider
        // request bytes depend on insertion order and silently break prefix
        // caching. Detect that here instead of in production.
        assert!(json_object_order_is_stable());
    }

    #[test]
    fn sorts_nested_object_keys_without_reordering_arrays() {
        let mut value = json!({
            "z": {"b": 2, "a": 1},
            "a": [{"d": 4, "c": 3}, "tail"]
        });

        sort_object_keys(&mut value);

        assert_eq!(
            value.to_string(),
            r#"{"a":[{"c":3,"d":4},"tail"],"z":{"a":1,"b":2}}"#
        );
    }

    #[test]
    fn downscales_oversized_user_images_deterministically() {
        let url = png_data_url(3000, 1500);
        let mut first = user_image_body(Value::String(url));
        let mut second = first.clone();

        let first_stats = normalize_provider_images(&mut first).unwrap();
        let second_stats = normalize_provider_images(&mut second).unwrap();

        assert_eq!(first, second);
        assert_eq!(first_stats.normalized_images, 1);
        assert_eq!(second_stats.normalized_images, 1);
        let (media_type, width, height) = normalized_dimensions(
            first["input"][0]["content"][0]["image_url"]
                .as_str()
                .unwrap(),
        );
        assert_eq!(media_type, "image/png");
        assert_eq!((width, height), (MAX_VISION_DIM, 784));
        assert!(first_stats.saved_bytes > 0);
    }

    #[test]
    fn downscales_oversized_jpeg_and_keeps_jpeg_encoding() {
        let mut body = user_image_body(Value::String(jpeg_data_url(2400, 3000)));

        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats.normalized_images, 1);
        let (media_type, width, height) = normalized_dimensions(
            body["input"][0]["content"][0]["image_url"]
                .as_str()
                .unwrap(),
        );
        assert_eq!(media_type, "image/jpeg");
        assert_eq!((width, height), (1254, MAX_VISION_DIM));
    }

    #[test]
    fn re_encodes_gif_and_webp_into_supported_vision_formats() {
        assert_eq!(vision_output_media_type("image/png"), "image/png");
        assert_eq!(vision_output_media_type("image/gif"), "image/png");
        assert_eq!(vision_output_media_type("image/jpeg"), "image/jpeg");
        assert_eq!(vision_output_media_type("image/webp"), "image/jpeg");
    }

    #[test]
    fn refuses_to_decode_pixel_bombs() {
        assert!(!exceeds_decode_budget(1568, 1568));
        assert!(!exceeds_decode_budget(10_000, 5_000));
        assert!(exceeds_decode_budget(10_000, 5_001));
        assert!(exceeds_decode_budget(u32::MAX, 2));
    }

    #[test]
    fn normalizes_object_form_image_urls() {
        let mut body = user_image_body(json!({"url": png_data_url(3000, 1500)}));

        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats.normalized_images, 1);
        let normalized = body["input"][0]["content"][0]["image_url"]["url"]
            .as_str()
            .unwrap();
        let (_, width, _) = normalized_dimensions(normalized);
        assert_eq!(width, MAX_VISION_DIM);
    }

    #[test]
    fn leaves_remote_image_urls_untouched() {
        let mut body = user_image_body(Value::String(
            "https://example.test/screenshot.png".to_owned(),
        ));
        let original = body.clone();

        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats, ImageNormalizationStats::default());
        assert_eq!(body, original);
    }

    #[test]
    fn rejects_image_data_urls_that_are_not_base64() {
        let mut body = user_image_body(Value::String(
            "data:image/png;base64,not-base64!!".to_owned(),
        ));

        let error = normalize_provider_images(&mut body).unwrap_err();

        assert!(
            error.to_string().contains("not valid base64"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rejects_images_above_the_hard_byte_cap() {
        let oversized = vec![0u8; MAX_IMAGE_BYTES + 1];

        let error = normalize_image_bytes("image/png", &oversized).unwrap_err();
        assert!(
            error.to_string().contains("must be between"),
            "unexpected error: {error}"
        );

        let empty = normalize_image_bytes("image/png", &[]).unwrap_err();
        assert!(
            empty.to_string().contains("must be between"),
            "unexpected error: {empty}"
        );
    }

    #[test]
    fn trailing_tool_output_run_covers_only_the_unseen_results() {
        let history = tool_history(
            json!([{"type":"input_text","text":"old"}]),
            json!([{"type":"input_text","text":"new"}]),
        );
        let input = history["input"].as_array().unwrap();
        assert_eq!(trailing_tool_output_run(input), 4..5);

        // A steering user message can follow the fresh results without hiding
        // them.
        let mut with_steering = input.clone();
        with_steering.push(
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":"stop"}]}),
        );
        assert_eq!(trailing_tool_output_run(&with_steering), 4..5);

        // Parallel tool calls return a contiguous run of outputs.
        let mut parallel = input.clone();
        parallel.push(json!({
            "type":"function_call_output","call_id":"call_new_2","output":[{"type":"input_text","text":"also new"}]
        }));
        assert_eq!(trailing_tool_output_run(&parallel), 4..6);

        // Once the model has answered them, the results are settled history.
        let mut answered = input.clone();
        answered.push(
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}),
        );
        assert_eq!(trailing_tool_output_run(&answered), 0..0);

        let mut reasoned = input.clone();
        reasoned.push(json!({"type":"reasoning","summary":[]}));
        assert_eq!(trailing_tool_output_run(&reasoned), 0..0);

        assert_eq!(trailing_tool_output_run(&[]), 0..0);
    }

    #[test]
    fn marks_replayed_tool_images_and_keeps_the_fresh_one() {
        let image_url = "data:image/png;base64,AAAA";
        let mut body = tool_history(
            json!([{"type":"input_image","image_url":image_url}]),
            json!([
                {"type":"input_text","text":"latest screenshot"},
                {"type":"input_image","image_url":image_url}
            ]),
        );

        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats.omitted_tool_images, 1);
        assert_eq!(stats.saved_bytes, image_url.len());
        assert_eq!(
            body["input"][1]["output"],
            json!([{"type":"input_text","text":TOOL_IMAGE_PLACEHOLDER}])
        );
        assert_eq!(
            body["input"][4]["output"],
            json!([
                {"type":"input_text","text":"latest screenshot"},
                {"type":"input_image","image_url":image_url}
            ])
        );
    }

    #[test]
    fn keeps_replayed_tool_text_and_marks_the_omitted_image() {
        let image_url = "data:image/png;base64,AAAA";
        let mut body = tool_history(
            json!([
                {"type":"input_text","text":"screenshot captured"},
                {"type":"input_image","image_url":image_url}
            ]),
            json!([{"type":"input_text","text":"no image here"}]),
        );

        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats.omitted_tool_images, 1);
        // Without the marker the model would read "screenshot captured" as the
        // complete tool result.
        assert_eq!(
            body["input"][1]["output"],
            json!([
                {"type":"input_text","text":"screenshot captured"},
                {"type":"input_text","text":TOOL_IMAGE_PLACEHOLDER}
            ])
        );
    }

    #[test]
    fn reports_omitted_tool_image_bytes_for_every_replayed_image() {
        let image_url = format!("data:image/png;base64,{}", "A".repeat(4_000));
        let mut body = tool_history(
            json!([
                {"type":"input_image","image_url":image_url},
                {"type":"input_image","image_url":{"url":image_url}}
            ]),
            json!([{"type":"input_text","text":"no image here"}]),
        );
        body["input"][1]["type"] = json!("custom_tool_call_output");

        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats.omitted_tool_images, 2);
        assert_eq!(stats.saved_bytes, image_url.len() * 2);
        assert_eq!(
            body["input"][1]["output"],
            json!([{"type":"input_text","text":TOOL_IMAGE_PLACEHOLDER}])
        );
    }

    #[test]
    fn downscales_fresh_tool_images_instead_of_dropping_them() {
        let mut body = tool_history(
            json!([{"type":"input_text","text":"old"}]),
            json!([{"type":"input_image","image_url":png_data_url(3000, 1500)}]),
        );

        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats.omitted_tool_images, 0);
        assert_eq!(stats.normalized_images, 1);
        let (media_type, width, height) =
            normalized_dimensions(body["input"][4]["output"][0]["image_url"].as_str().unwrap());
        assert_eq!(media_type, "image/png");
        assert_eq!((width, height), (MAX_VISION_DIM, 784));
    }

    #[test]
    fn marks_fresh_tool_images_that_cannot_be_normalized() {
        let mut body = tool_history(
            json!([{"type":"input_text","text":"old"}]),
            json!([
                {"type":"input_text","text":"latest screenshot"},
                {"type":"input_image","image_url":"data:image/png;base64,not-base64!!"}
            ]),
        );

        // A user-attached image fails loudly; a tool observation degrades to the
        // marker so one screenshot cannot break the turn.
        let stats = normalize_provider_images(&mut body).unwrap();

        assert_eq!(stats.omitted_tool_images, 1);
        assert_eq!(
            body["input"][4]["output"],
            json!([
                {"type":"input_text","text":"latest screenshot"},
                {"type":"input_text","text":TOOL_IMAGE_PLACEHOLDER}
            ])
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn image_work_is_limited_to_two_concurrent_tasks() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();

        for _ in 0..8 {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            tasks.push(tokio::spawn(async move {
                run_image_work(move || {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
            }));
        }

        for task in tasks {
            task.await.unwrap().unwrap();
        }

        assert_eq!(
            peak.load(Ordering::SeqCst),
            MAX_CONCURRENT_IMAGE_NORMALIZATIONS
        );
    }
}
