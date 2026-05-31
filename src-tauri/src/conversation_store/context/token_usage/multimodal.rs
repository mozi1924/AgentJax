use serde_json::Value;

pub(super) fn is_image_part(part: &Value) -> bool {
    let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
    part_type == "image_url"
        || part_type == "input_image"
        || part_type == "image"
        || part.get("image_url").is_some()
}

pub(super) fn estimate_image_tokens(part: &Value) -> usize {
    let detail = image_detail(part);
    if detail.as_deref() == Some("low") {
        return 85;
    }

    let dimensions = image_dimensions(part);
    if let Some((width, height)) = dimensions {
        let tiles = ceil_div(width, 512).saturating_mul(ceil_div(height, 512));
        return 85usize.saturating_add(tiles.saturating_mul(170));
    }

    // Without dimensions, the safest deterministic fallback is the low-detail
    // floor documented by image-capable OpenAI APIs.
    85
}

fn image_detail(part: &Value) -> Option<String> {
    part.get("detail")
        .and_then(Value::as_str)
        .or_else(|| {
            part.get("image_url")
                .and_then(Value::as_object)
                .and_then(|image| image.get("detail"))
                .and_then(Value::as_str)
        })
        .map(|value| value.to_ascii_lowercase())
}

fn image_dimensions(part: &Value) -> Option<(usize, usize)> {
    let width = part.get("width").and_then(Value::as_u64).or_else(|| {
        part.get("image_url")
            .and_then(Value::as_object)
            .and_then(|image| image.get("width"))
            .and_then(Value::as_u64)
    })? as usize;
    let height = part.get("height").and_then(Value::as_u64).or_else(|| {
        part.get("image_url")
            .and_then(Value::as_object)
            .and_then(|image| image.get("height"))
            .and_then(Value::as_u64)
    })? as usize;
    (width > 0 && height > 0).then_some((width, height))
}

fn ceil_div(value: usize, divisor: usize) -> usize {
    value.saturating_add(divisor.saturating_sub(1)) / divisor
}
