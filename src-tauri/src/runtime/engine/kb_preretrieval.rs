//! KB pre-retrieval — automatically searches the agent's knowledge bases
//! before each turn and injects relevant context as a system item.
//!
//! Extracted from the monolithic `run_turn()` in `engine.rs`.

use crate::config::{AgentConfig, AppConfig};
use crate::rag::KnowledgeBaseManager;
use serde_json::Value;

/// Search accessible knowledge bases and return a system item containing
/// the top results, or `None` if no relevant content is found.
pub(crate) async fn build_kb_context_item(
    config: &AppConfig,
    _agent: &AgentConfig,
    agent_id: &str,
    user_query: &str,
) -> Option<Value> {
    if !config.rag.enabled || config.rag.knowledge_bases.is_empty() {
        return None;
    }
    if user_query.trim().is_empty() {
        return None;
    }

    let kb_manager = match KnowledgeBaseManager::from_config(config) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("Pre-retrieval KB manager init failed: {e}");
            return None;
        }
    };

    let accessible_kbs = match kb_manager.list_kbs_filtered(config, agent_id).await {
        Ok(kbs) => kbs,
        Err(e) => {
            log::warn!("Pre-retrieval KB listing failed: {e}");
            return None;
        }
    };

    let max_total_chunks: usize = 5;
    let per_kb = 3usize;
    let mut all_chunks: Vec<String> = Vec::new();

    for kb_info in accessible_kbs.iter() {
        if all_chunks.len() >= max_total_chunks {
            break;
        }
        let remaining = max_total_chunks - all_chunks.len();
        let k = per_kb.min(remaining);
        match kb_manager.search(&kb_info.id, user_query, k, config).await {
            Ok(results) => {
                for r in results {
                    all_chunks.push(format!(
                        "[Knowledge Base] \"{}\" — \"{}\" (score: {:.2}):\n---\n{}\n---",
                        kb_info.name,
                        r.title,
                        r.score,
                        r.content.trim()
                    ));
                }
            }
            Err(e) => {
                log::warn!(
                    "Pre-retrieval search failed for KB '{}': {}",
                    kb_info.id,
                    e
                );
            }
        }
    }

    if all_chunks.is_empty() {
        return None;
    }

    let kbc = all_chunks.join("\n\n");
    Some(serde_json::json!({
        "role": "system",
        "content": [
            {
                "type": "input_text",
                "text": format!(
                    "[Knowledge Base Context — automatically retrieved]\n\
                     The following excerpts are from your knowledge bases \
                     and may be relevant to the user's query. Use them to \
                     inform your answer, but prioritize the user's actual \
                     question over the retrieved context.\n\n{}",
                    kbc
                )
            }
        ]
    }))
}
