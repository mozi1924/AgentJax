use super::chat_utils::chrono_like_now_id;
use crate::sub_agents::manager::SubAgentManager;
use crate::tools::background_jobs;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tokio::sync::watch;

#[derive(Debug, Clone)]
pub struct ActiveChatRequest {
    pub conversation_id: String,
    pub cancel_tx: watch::Sender<bool>,
}

#[derive(Debug, Clone)]
struct ActiveTitleRequest {
    job_id: String,
    cancel_tx: watch::Sender<bool>,
}

#[derive(Default)]
pub struct ChatRequestRegistry {
    requests: Mutex<HashMap<String, ActiveChatRequest>>,
    title_requests: Mutex<HashMap<String, ActiveTitleRequest>>,
    deleted_conversations: Mutex<HashSet<String>>,
}

impl ChatRequestRegistry {
    pub fn try_register_chat_request(
        &self,
        request_id: String,
        conversation_id: String,
        cancel_tx: watch::Sender<bool>,
    ) -> Result<bool, String> {
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| "Failed to lock chat request registry".to_string())?;

        if requests
            .values()
            .any(|request| request.conversation_id == conversation_id)
        {
            return Ok(false);
        }
        if requests.contains_key(&request_id) {
            return Err(format!(
                "Request id '{}' already exists in chat request registry",
                request_id
            ));
        }

        requests.insert(
            request_id,
            ActiveChatRequest {
                conversation_id,
                cancel_tx,
            },
        );
        Ok(true)
    }

    pub fn remove_chat_request(
        &self,
        request_id: &str,
    ) -> Result<Option<ActiveChatRequest>, String> {
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| "Failed to lock chat request registry".to_string())?;
        Ok(requests.remove(request_id))
    }

    pub fn cancel_chat_request(&self, request_id: &str) -> Result<bool, String> {
        let active_request = {
            let requests = self
                .requests
                .lock()
                .map_err(|_| "Failed to lock chat request registry".to_string())?;
            requests.get(request_id).cloned()
        };

        if let Some(request) = active_request {
            request
                .cancel_tx
                .send(true)
                .map_err(|_| "Failed to signal chat stream cancellation".to_string())?;
            // Background sidecar jobs are deliberately detached from the
            // provider turn, so user-initiated Stop must cancel them
            // explicitly or they can outlive the visible request.
            background_jobs::cancel_conversation_jobs(&request.conversation_id);
            SubAgentManager::cancel_conversation_agents(&request.conversation_id);
            return Ok(true);
        }

        Ok(false)
    }

    pub fn register_title_request(
        &self,
        conversation_id: &str,
        cancel_tx: watch::Sender<bool>,
    ) -> Result<String, String> {
        let job_id = format!("title-{}-{}", conversation_id, chrono_like_now_id());
        let previous = {
            let mut title_requests = self
                .title_requests
                .lock()
                .map_err(|_| "Failed to lock title request registry".to_string())?;

            title_requests.insert(
                conversation_id.to_string(),
                ActiveTitleRequest {
                    job_id: job_id.clone(),
                    cancel_tx,
                },
            )
        };

        if let Some(previous) = previous {
            let _ = previous.cancel_tx.send(true);
        }

        Ok(job_id)
    }

    pub fn finish_title_request(&self, conversation_id: &str, job_id: &str) -> Result<(), String> {
        let mut title_requests = self
            .title_requests
            .lock()
            .map_err(|_| "Failed to lock title request registry".to_string())?;

        let should_remove = title_requests
            .get(conversation_id)
            .map(|request| request.job_id == job_id)
            .unwrap_or(false);

        if should_remove {
            title_requests.remove(conversation_id);
        }

        Ok(())
    }

    pub fn cancel_title_request(&self, conversation_id: &str) -> Result<bool, String> {
        let request = {
            let mut title_requests = self
                .title_requests
                .lock()
                .map_err(|_| "Failed to lock title request registry".to_string())?;
            title_requests.remove(conversation_id)
        };

        if let Some(request) = request {
            let _ = request.cancel_tx.send(true);
            return Ok(true);
        }

        Ok(false)
    }

    pub fn mark_conversation_deleted(&self, conversation_id: &str) -> Result<(), String> {
        let mut deleted_conversations = self
            .deleted_conversations
            .lock()
            .map_err(|_| "Failed to lock deleted conversation registry".to_string())?;
        deleted_conversations.insert(conversation_id.to_string());
        Ok(())
    }

    pub fn clear_conversation_deleted(&self, conversation_id: &str) -> Result<(), String> {
        let mut deleted_conversations = self
            .deleted_conversations
            .lock()
            .map_err(|_| "Failed to lock deleted conversation registry".to_string())?;
        deleted_conversations.remove(conversation_id);
        Ok(())
    }

    pub fn is_conversation_deleted(&self, conversation_id: &str) -> Result<bool, String> {
        let deleted_conversations = self
            .deleted_conversations
            .lock()
            .map_err(|_| "Failed to lock deleted conversation registry".to_string())?;
        Ok(deleted_conversations.contains(conversation_id))
    }

    pub fn cancel_conversation_tasks(&self, conversation_id: &str) -> Result<(), String> {
        let chat_cancel_txs = {
            let mut requests = self
                .requests
                .lock()
                .map_err(|_| "Failed to lock chat request registry".to_string())?;

            let request_ids = requests
                .iter()
                .filter_map(|(request_id, request)| {
                    if request.conversation_id == conversation_id {
                        Some(request_id.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            let mut cancel_txs = Vec::with_capacity(request_ids.len());
            for request_id in request_ids {
                if let Some(request) = requests.remove(&request_id) {
                    cancel_txs.push(request.cancel_tx);
                }
            }

            cancel_txs
        };

        for cancel_tx in chat_cancel_txs {
            let _ = cancel_tx.send(true);
        }

        let _ = self.cancel_title_request(conversation_id)?;
        background_jobs::cancel_conversation_jobs(conversation_id);
        SubAgentManager::cancel_conversation_agents(conversation_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ChatRequestRegistry;
    use std::sync::Arc;
    use std::thread;
    use tokio::sync::watch;

    #[test]
    fn try_register_is_atomic_for_same_conversation() {
        let registry = Arc::new(ChatRequestRegistry::default());
        let barrier = Arc::new(std::sync::Barrier::new(8));

        let mut handles = Vec::new();
        for idx in 0..8usize {
            let registry = registry.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                let (cancel_tx, _rx) = watch::channel(false);
                barrier.wait();
                registry
                    .try_register_chat_request(
                        format!("req-{idx}"),
                        "conv-shared".to_string(),
                        cancel_tx,
                    )
                    .expect("register should not error")
            }));
        }

        let mut success_count = 0usize;
        for handle in handles {
            if handle.join().expect("thread should join") {
                success_count += 1;
            }
        }
        assert_eq!(success_count, 1, "only one request can own a conversation");
    }

    #[test]
    fn allows_parallel_registration_for_different_conversations() {
        let registry = Arc::new(ChatRequestRegistry::default());
        let mut handles = Vec::new();

        for idx in 0..8usize {
            let registry = registry.clone();
            handles.push(thread::spawn(move || {
                let (cancel_tx, _rx) = watch::channel(false);
                registry
                    .try_register_chat_request(
                        format!("req-{idx}"),
                        format!("conv-{idx}"),
                        cancel_tx,
                    )
                    .expect("register should succeed")
            }));
        }

        for handle in handles {
            assert!(handle.join().expect("thread should join"));
        }
    }
}
