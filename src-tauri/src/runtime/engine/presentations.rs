use crate::provider_api::types::ProviderStreamEvent;
use crate::tools::{ToolCatalogSnapshot, ToolPresentation};

pub(super) fn merge_tool_presentations(
    existing: Option<ToolPresentation>,
    fallback: Option<ToolPresentation>,
) -> Option<ToolPresentation> {
    match (existing, fallback) {
        (Some(mut existing), Some(fallback)) => {
            if existing.display_name.trim().is_empty() {
                existing.display_name = fallback.display_name;
            }
            if existing.description.trim().is_empty() {
                existing.description = fallback.description;
            }
            let icon_missing = existing
                .icon
                .as_deref()
                .map(str::trim)
                .map(|icon| icon.is_empty())
                .unwrap_or(true);
            if icon_missing {
                existing.icon = fallback.icon;
            }
            Some(existing)
        }
        (Some(existing), None) => Some(existing),
        (None, fallback) => fallback,
    }
}

fn enrich_tool_presentation(
    existing: Option<ToolPresentation>,
    snapshot: &ToolCatalogSnapshot,
    tool_name: &str,
) -> Option<ToolPresentation> {
    merge_tool_presentations(existing, snapshot.presentation_for(tool_name).cloned())
}

pub(super) fn enrich_tool_stream_event(
    event: ProviderStreamEvent,
    snapshot: &ToolCatalogSnapshot,
) -> ProviderStreamEvent {
    match event {
        ProviderStreamEvent::ToolCallStarted {
            item_id,
            call_id,
            name,
            presentation,
        } => ProviderStreamEvent::ToolCallStarted {
            presentation: enrich_tool_presentation(presentation, snapshot, &name),
            item_id,
            call_id,
            name,
        },
        ProviderStreamEvent::ToolCallCompleted {
            item_id,
            call_id,
            name,
            arguments,
            presentation,
        } => ProviderStreamEvent::ToolCallCompleted {
            presentation: enrich_tool_presentation(presentation, snapshot, &name),
            item_id,
            call_id,
            name,
            arguments,
        },
        ProviderStreamEvent::ToolCallProgress {
            call_id,
            name,
            elapsed_ms,
            presentation,
        } => ProviderStreamEvent::ToolCallProgress {
            presentation: enrich_tool_presentation(presentation, snapshot, &name),
            call_id,
            name,
            elapsed_ms,
        },
        other => other,
    }
}
