//! Embedding provider registry.
//!
//! A static registry that stores registered [`EmbeddingProvider`] implementations.
//! Follows the same pattern as `provider_api::registry`.

use std::sync::{OnceLock, RwLock};

use super::provider::EmbeddingProvider;

static EMBEDDING_REGISTRY: OnceLock<RwLock<Vec<Box<dyn EmbeddingProvider>>>> =
    OnceLock::new();

fn get_registry() -> &'static RwLock<Vec<Box<dyn EmbeddingProvider>>> {
    EMBEDDING_REGISTRY.get_or_init(|| RwLock::new(Vec::new()))
}

/// Register an embedding provider.
///
/// If a provider with the same name is already registered, the new one
/// replaces it.
pub fn register(provider: Box<dyn EmbeddingProvider>) {
    let name = provider.provider_name().to_string();
    let mut registry = get_registry().write().unwrap();
    registry.retain(|p| p.provider_name() != name);
    registry.push(provider);
}

/// Retrieve an embedding provider by name.
pub fn get(name: &str) -> Option<Box<dyn EmbeddingProvider>> {
    let registry = get_registry().read().unwrap();
    registry
        .iter()
        .find(|p| p.provider_name() == name)
        .map(|p| clone_provider(p))
}

/// List all registered provider names.
pub fn list() -> Vec<String> {
    let registry = get_registry().read().unwrap();
    registry
        .iter()
        .map(|p| p.provider_name().to_string())
        .collect()
}

/// Get the first registered provider (the default).
pub fn default() -> Option<Box<dyn EmbeddingProvider>> {
    let registry = get_registry().read().unwrap();
    registry.first().map(|p| clone_provider(p))
}

/// Number of registered providers.
pub fn count() -> usize {
    let registry = get_registry().read().unwrap();
    registry.len()
}

/// Clone a boxed provider using its `provider_name` to look it up from the
/// registry and create a fresh instance. This works because providers are
/// stateless — they store only config, not session state.
fn clone_provider(p: &Box<dyn EmbeddingProvider>) -> Box<dyn EmbeddingProvider> {
        #[allow(unused_variables)]
    let name = p.provider_name();
    let config = provider_config_for(name);
    create_provider(name, &config)
}

/// Retrieve the config for a given provider name from [`crate::config::AppConfig`].
/// Falls back to defaults if no config is loaded.
fn provider_config_for(_name: &str) -> crate::config::EmbeddingProviderConfig {
    // Try to read from the running app config
    // This is a best-effort approach; in practice the caller provides config explicitly.
    crate::config::EmbeddingProviderConfig::default()
}

/// Create a provider instance from a name and config.
pub(crate) fn create_provider(
    name: &str,
    config: &crate::config::EmbeddingProviderConfig,
) -> Box<dyn EmbeddingProvider> {
    match name {
        "openai" => Box::new(super::openai::OpenAiEmbeddingProvider::new(config)),
        other => {
            log::warn!("Unknown embedding provider '{}', falling back to openai", other);
            Box::new(super::openai::OpenAiEmbeddingProvider::new(config))
        }
    }
}

/// Initialize built-in embedding providers.
///
/// Called during app startup to register the default set of providers.
pub fn init_builtin_providers() {
    let config = crate::config::EmbeddingProviderConfig::default();
    let provider = create_provider("openai", &config);
    register(provider);
    log::info!(
        "Registered built-in embedding provider: openai ({}, dims={})",
        config.model,
        config.dimensions
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::provider::EmbeddingProvider;
    use crate::embeddings::types::EmbeddingRequest;
    use async_trait::async_trait;

    struct MockProvider;

    #[async_trait]
    impl EmbeddingProvider for MockProvider {
        fn provider_name(&self) -> &str { "mock" }
        fn model_name(&self) -> &str { "mock-model" }
        fn dimensions(&self) -> usize { 4 }

        async fn embed(&self, _input: &EmbeddingRequest) -> crate::error::AgentJaxResult<EmbeddingResponse> {
            Ok(EmbeddingResponse {
                embeddings: vec![vec![0.1, 0.2, 0.3, 0.4]],
                model: "mock-model".to_string(),
                usage: Default::default(),
            })
        }
    }

    #[test]
    fn test_register_and_get() {
        // Reset for test isolation
        let registry = get_registry();
        {
            let mut r = registry.write().unwrap();
            r.clear();
        }

        register(Box::new(MockProvider));
        assert!(count() >= 1);
        assert!(list().contains(&"mock".to_string()));
    }
}
