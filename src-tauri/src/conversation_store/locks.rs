use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

type ConversationMutex = Arc<Mutex<()>>;

fn lock_registry() -> &'static Mutex<BTreeMap<String, ConversationMutex>> {
    static LOCKS: OnceLock<Mutex<BTreeMap<String, ConversationMutex>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn conversation_lock(conversation_id: &str) -> Result<ConversationMutex, String> {
    let mut registry = lock_registry()
        .lock()
        .map_err(|_| "Conversation lock registry is poisoned".to_string())?;
    Ok(registry
        .entry(conversation_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
}

pub fn with_conversation_lock<T, F>(conversation_id: &str, action: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String>,
{
    let lock = conversation_lock(conversation_id)?;
    let _guard = lock
        .lock()
        .map_err(|_| format!("Conversation lock is poisoned for '{conversation_id}'"))?;
    action()
}
