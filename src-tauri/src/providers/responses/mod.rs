pub mod models;
pub mod stream;

pub fn normalize_reasoning_levels(levels: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();

    for level in levels {
        let level = level.trim().to_lowercase();
        if !matches!(
            level.as_str(),
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh"
        ) {
            continue;
        }

        if !normalized.iter().any(|existing| existing == &level) {
            normalized.push(level);
        }
    }

    normalized
}
