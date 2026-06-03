use crate::tools::{MountedToolSourceSessions, ToolCatalogStateChange};

/// Apply tool-catalog side effects emitted by control tools between provider hops.
pub(super) fn apply_tool_state_changes(
    mounted_tool_sources: &mut MountedToolSourceSessions,
    state_changes: Vec<ToolCatalogStateChange>,
) {
    for state_change in state_changes {
        match state_change {
            ToolCatalogStateChange::MountToolSource(source_session) => {
                mounted_tool_sources.insert(source_session.source_id.clone(), *source_session);
            }
            ToolCatalogStateChange::UnmountToolSource { source_id, .. } => {
                mounted_tool_sources.remove(&source_id);
            }
        }
    }
}
