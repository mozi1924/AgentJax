//! Smoke tests for the RAG / Knowledge Base system.
//!
//! These tests require a running Ollama instance with an embedding model.
//! They are ignored (`#[ignore]`) by default and must be run explicitly:
//!
//! ```sh
//! cargo test -- --ignored rag_smoke
//! ```
//!
//! ## Prerequisites
//!
//! 1. Install and start [Ollama](https://ollama.ai)
//! 2. Pull an embedding model: `ollama pull nomic-embed-text`
//! 3. Run the test with the `AGENTJAX_HOME` env var pointing to a temp dir:
//!
//! ```sh
//! AGENTJAX_HOME=/tmp/agentjax-test cargo test -- --ignored rag_smoke
//! ```

#![cfg(test)]

use crate::agentjax_home::AGENTJAX_HOME_ENV;
use crate::config::{
    ensure_default_agent_profile, init_config_if_missing, serialize_config_to_yaml, AppConfig,
    EmbeddingProviderConfig, ProviderConfig, RagConfig,
};
use crate::rag::types::Document;
use crate::rag::KnowledgeBaseManager;
use std::collections::BTreeMap;
use std::sync::Once;
use std::time::Duration;

static RUSTLS_CRYPTO_PROVIDER: Once = Once::new();

fn ensure_rustls_crypto_provider() {
    RUSTLS_CRYPTO_PROVIDER.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install rustls ring crypto provider");
    });
}

struct TestHomeGuard {
    home: std::path::PathBuf,
}

impl Drop for TestHomeGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup_test_home() -> TestHomeGuard {
    let home =
        std::env::temp_dir().join(format!("agentjax-rag-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).expect("create test home");
    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }
    TestHomeGuard { home }
}

/// Configure a test RagConfig that points to Ollama for embeddings.
fn test_rag_config(embedding_model: &str) -> RagConfig {
    // Default dimensions for known models; override via env var if needed
    let dimensions: usize = std::env::var("AGENTJAX_EMBEDDING_DIMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1024);

    RagConfig {
        enabled: true,
        chunk_size: 500,
        chunk_overlap: 50,
        chunk_window: Some(200),
        top_k: 5,
        embedding: EmbeddingProviderConfig {
            // Format: "{provider_key}/{model_id}"
            // The provider_key "ollama" must match a key in AppConfig.providers
            model: format!("ollama/{embedding_model}"),
            dimensions,
        },
        knowledge_bases: BTreeMap::new(),
        embedding_batch_size: 10,
        embedding_concurrency: 1,
        embedding_batch_throttle_ms: 500,
    }
}

/// Test config with Ollama as the embedding provider.
fn test_app_config(embedding_model: &str) -> AppConfig {
    let mut providers = BTreeMap::new();
    providers.insert(
        "ollama".to_string(),
        ProviderConfig {
            kind: "openai-compatible".to_string(),
            api_endpoint: "http://localhost:11434/v1".to_string(),
            models: {
                let mut models = BTreeMap::new();
                // Explicitly set api_protocol for the embedding model
                models.insert(
                    embedding_model.to_string(),
                    crate::config::ProviderModelConfig {
                        enabled: true,
                        name: None,
                        api_protocol: Some("embeddings".to_string()),
                        kind: Some("embedding".to_string()),
                        request: crate::config::ModelRequestConfig::default(),
                    },
                );
                models
            },
            ..Default::default()
        },
    );

    AppConfig {
        language: "en".to_string(),
        active_agent_id: "main".to_string(),
        providers,
        rag: test_rag_config(embedding_model),
        ..Default::default()
    }
}

/// Write a minimal config.yaml and agent.yaml to the test home directory.
fn init_test_config(app_config: &AppConfig) {
    let config_path = init_config_if_missing().expect("init config");
    let yaml = serialize_config_to_yaml(app_config).expect("serialize config");
    std::fs::write(&config_path, &yaml).expect("write config.yaml");
    ensure_default_agent_profile().expect("create default agent");
}

// ── Smoke Test ──────────────────────────────────────────────────────────────

/// The embedding model to use for the smoke test.
/// Override with environment variable `AGENTJAX_EMBEDDING_MODEL`.
fn embedding_model() -> String {
    std::env::var("AGENTJAX_EMBEDDING_MODEL")
        .unwrap_or_else(|_| "bge-m3:latest".to_string())
}

/// Check if Ollama is running by pinging the health endpoint.
async fn ollama_is_available() -> bool {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok();

    match client {
        Some(client) => match client.get("http://localhost:11434/api/tags").send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        },
        None => false,
    }
}

/// Smoke test: full embedding → index → search pipeline.
///
/// 1. Creates a temporary AgentJax home with Ollama config
/// 2. Initializes KnowledgeBaseManager
/// 3. Indexes a test markdown document
/// 4. Searches for the indexed content
/// 5. Verifies results contain the expected content
///
/// Run with: `cargo test -- --ignored rag_smoke_test`
#[tokio::test]
#[ignore = "Requires Ollama running with nomic-embed-text"]
async fn rag_smoke_test() {
    ensure_rustls_crypto_provider();

    if !ollama_is_available().await {
        eprintln!("⚠ Ollama is not running at http://localhost:11434 — skipping smoke test");
        eprintln!("  Start Ollama and pull an embedding model:");
        eprintln!("  ollama pull {}", embedding_model());
        return;
    }

    let _guard = setup_test_home();
    let model = embedding_model();

    // Setup config
    let app_config = test_app_config(&model);
    init_test_config(&app_config);

    // Reload config from disk to ensure the full config pipeline works
    let full_config = crate::config::load_active_config().expect("load active config");

    // Initialize KnowledgeBaseManager
    let kb_manager =
        KnowledgeBaseManager::from_config(&full_config.shared)
            .expect("create KB manager");

    // Create a test knowledge base
    let kb_id = "smoke-test";
    kb_manager.open_kb(kb_id).await.expect("open KB");

    // ── Index a test document ───────────────────────────────────────────

    let test_content = r#"
# AgentJax 知识库系统测试

AgentJax 是一个强大的本地 AI 代理运行时。
它支持多种 AI 提供商、MCP 工具和知识库检索。

## 核心功能

- **RAG（检索增强生成）**：通过向量和关键词混合搜索，
  让 AI 代理可以查询本地文档内容。
- **知识库管理**：支持 Markdown 文件索引，
  可配置文件或文件夹路径作为知识来源。
- **作用域控制**：通过 `disabled_agents` 字段，
  可以控制哪些 agent profile 可以访问特定知识库。

AgentJax 使用 LanceDB 作为向量数据库，
SQLite FTS5 作为全文搜索引擎。
这种混合架构提供了最佳的检索质量。
"#;

    let document = Document {
        id: "agentjax-intro".to_string(),
        content: test_content.to_string(),
        metadata: {
            let mut m = BTreeMap::new();
            m.insert("title".to_string(), "AgentJax 知识库系统测试".to_string());
            m
        },
    };

    // Index the document — this is the real end-to-end test
    let progress = kb_manager
        .index_document(kb_id, document, &full_config.shared)
        .await
        .expect("index document");

    eprintln!(
        "✓ Indexed document: {} chunks created",
        progress.chunks_created
    );
    assert!(
        progress.chunks_created > 0,
        "Expected at least 1 chunk, got {}",
        progress.chunks_created
    );

    // ── Search for content ──────────────────────────────────────────────

    // Search for something that should match
    let results = kb_manager
        .search(kb_id, "LanceDB 向量数据库", 5, 0, &full_config.shared)
        .await
        .expect("search KB");

    eprintln!("✓ Search returned {} results", results.len());
    assert!(
        !results.is_empty(),
        "Expected at least 1 search result"
    );

    // Check that the top result contains expected content
    let top = &results[0];
    assert!(
        top.content.contains("LanceDB") || top.content.contains("向量"),
        "Top result should contain content about LanceDB or 向量数据库.\nGot: {}",
        top.content
    );
    assert!(
        top.score > 0.0,
        "Expected positive score, got {}",
        top.score
    );

    eprintln!("✓ Top result score: {:.4}", top.score);
    eprintln!("✓ Top result: {}", top.content.chars().take(100).collect::<String>());

    // ── Verify keyword search also works ────────────────────────────────

    let kw_results = kb_manager
        .search(kb_id, "SQLite FTS5 全文搜索", 3, 0, &full_config.shared)
        .await
        .expect("keyword search KB");

    assert!(
        !kw_results.is_empty(),
        "Expected keyword search results"
    );

    let kw_top = &kw_results[0];
    assert!(
        kw_top.keyword_score > 0.0 || kw_top.vector_score > 0.0,
        "Expected positive score in keyword search"
    );

    eprintln!("✓ Keyword search returned {} results", kw_results.len());
    eprintln!("✓ All smoke tests passed! 🎉");
}

/// Minimal test: just verify embedding endpoint responds.
/// Run this first to check connectivity:
/// `cargo test -- --ignored rag_embedding_connectivity`
#[tokio::test]
#[ignore = "Requires Ollama running"]
async fn rag_embedding_connectivity() {
    ensure_rustls_crypto_provider();

    if !ollama_is_available().await {
        eprintln!("⚠ Ollama is not available — skipping");
        return;
    }

    let _guard = setup_test_home();
    let model = embedding_model();

    let app_config = test_app_config(&model);
    init_test_config(&app_config);
    let full_config = crate::config::load_active_config().expect("load active config");

    // Directly call the embedding API to test connectivity
    // Use the resolved provider and model ID via the embedding profile
    let (provider_key, _provider, model_id) = full_config
        .shared
        .resolve_embedding_profile(&full_config.shared.rag.embedding.model)
        .expect("resolve embedding profile");
    let response = crate::provider_api::embed_text(
        &full_config.shared,
        &provider_key,
        &model_id,
        &crate::provider_api::EmbeddingRequest::single("Hello, world!"),
    )
    .await
    .expect("embedding should succeed");

    assert_eq!(response.embeddings.len(), 1, "should get 1 embedding vector");
    assert!(
        !response.embeddings[0].is_empty(),
        "embedding vector should not be empty"
    );

    eprintln!(
        "✓ Ollama embedding response: {} dimensions, model={}",
        response.embeddings[0].len(),
        response.model
    );
}
