use super::types::ConversationSummary;
use crate::agentjax_err;
use crate::error::AgentJaxResult;
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

type ConversationMutex = Arc<Mutex<()>>;

#[derive(Debug, Clone, Default)]
struct ConversationIndexEntry {
    line_ids: Option<HashSet<String>>,
    summary: Option<ConversationSummary>,
}

fn lock_registry() -> &'static Mutex<BTreeMap<String, ConversationMutex>> {
    static LOCKS: OnceLock<Mutex<BTreeMap<String, ConversationMutex>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn conversation_lock(conversation_id: &str) -> AgentJaxResult<ConversationMutex> {
    let mut registry = lock_registry()
        .lock()
        .map_err(|_| agentjax_err!("Conversation lock registry is poisoned", Internal))?;
    Ok(registry
        .entry(conversation_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
}

fn conversation_index_registry() -> &'static Mutex<BTreeMap<String, ConversationIndexEntry>> {
    static INDEX: OnceLock<Mutex<BTreeMap<String, ConversationIndexEntry>>> = OnceLock::new();
    INDEX.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn with_conversation_lock<T, F>(conversation_id: &str, action: F) -> AgentJaxResult<T>
where
    F: FnOnce() -> AgentJaxResult<T>,
{
    let lock = conversation_lock(conversation_id)?;
    let _guard = lock.lock().map_err(|_| {
        agentjax_err!(
            format!("Conversation lock is poisoned for '{conversation_id}'"),
            Internal
        )
    })?;
    action()
}

pub fn cached_line_id_exists(conversation_id: &str, line_id: &str) -> AgentJaxResult<Option<bool>> {
    let registry = conversation_index_registry()
        .lock()
        .map_err(|_| agentjax_err!("Conversation index registry is poisoned", Internal))?;
    Ok(registry
        .get(conversation_id)
        .and_then(|entry| entry.line_ids.as_ref())
        .map(|line_ids| line_ids.contains(line_id)))
}

pub fn replace_cached_line_ids(
    conversation_id: &str,
    line_ids: HashSet<String>,
) -> AgentJaxResult<()> {
    let mut registry = conversation_index_registry()
        .lock()
        .map_err(|_| agentjax_err!("Conversation index registry is poisoned", Internal))?;
    registry
        .entry(conversation_id.to_string())
        .or_default()
        .line_ids = Some(line_ids);
    Ok(())
}

pub fn insert_cached_line_id(conversation_id: &str, line_id: &str) -> AgentJaxResult<()> {
    let mut registry = conversation_index_registry()
        .lock()
        .map_err(|_| agentjax_err!("Conversation index registry is poisoned", Internal))?;
    registry
        .entry(conversation_id.to_string())
        .or_default()
        .line_ids
        .get_or_insert_with(HashSet::new)
        .insert(line_id.to_string());
    Ok(())
}

pub fn cached_summary(conversation_id: &str) -> AgentJaxResult<Option<ConversationSummary>> {
    let registry = conversation_index_registry()
        .lock()
        .map_err(|_| agentjax_err!("Conversation index registry is poisoned", Internal))?;
    Ok(registry
        .get(conversation_id)
        .and_then(|entry| entry.summary.clone()))
}

pub fn replace_cached_summary(
    conversation_id: &str,
    summary: ConversationSummary,
) -> AgentJaxResult<()> {
    let mut registry = conversation_index_registry()
        .lock()
        .map_err(|_| agentjax_err!("Conversation index registry is poisoned", Internal))?;
    registry
        .entry(conversation_id.to_string())
        .or_default()
        .summary = Some(summary);
    Ok(())
}

pub fn invalidate_cached_conversation_index(conversation_id: &str) -> AgentJaxResult<()> {
    let mut registry = conversation_index_registry()
        .lock()
        .map_err(|_| agentjax_err!("Conversation index registry is poisoned", Internal))?;
    registry.remove(conversation_id);
    Ok(())
}
