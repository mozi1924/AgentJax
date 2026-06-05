#![recursion_limit = "512"]

mod agentjax_home;
mod commands;
pub mod config;
mod conversation_store;
mod conversation_store_utils;
pub(crate) mod error;
mod error_classifier;
pub(crate) mod lcm;
pub(crate) mod mcp;
pub(crate) mod memory;
mod message_phase;
mod models;
pub(crate) mod plugin_runtime;
pub(crate) mod provider_api;
pub(crate) mod rag;
pub(crate) mod runtime;
pub(crate) mod street;
pub(crate) mod sub_agents;
mod time_context;
pub(crate) mod tools;

use tauri::Manager;

#[cfg(test)]
mod providers_tests;
#[cfg(test)]
mod tools_tests;

fn parse_rust_log_level() -> log::LevelFilter {
    let default_level = if cfg!(debug_assertions) {
        log::LevelFilter::Info
    } else {
        log::LevelFilter::Warn
    };

    let raw = match std::env::var("RUST_LOG") {
        Ok(value) => value,
        Err(_) => return default_level,
    };

    let directive = raw
        .split(',')
        .map(str::trim)
        .find(|part| !part.is_empty())
        .unwrap_or("");

    let level_part = directive
        .rsplit_once('=')
        .map(|(_, level)| level.trim())
        .unwrap_or(directive)
        .to_ascii_lowercase();

    match level_part.as_str() {
        "off" => log::LevelFilter::Off,
        "error" => log::LevelFilter::Error,
        "warn" | "warning" => log::LevelFilter::Warn,
        "info" => log::LevelFilter::Info,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => default_level,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Install a rustls crypto provider before any TLS connections are made.
    // reqwest, tokio-tungstenite, and rmcp all use rustls under the hood.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls ring crypto provider");

    let log_level = parse_rust_log_level();

    tauri::Builder::default()
        .manage(commands::chat::ChatRequestRegistry::default())
        .manage(std::sync::Arc::new(crate::mcp::McpManager::new()))
        .manage(std::sync::Arc::new(
            commands::config::ConfigEventState::default(),
        ))
        .manage(
            commands::agents::AgentRegistryState::new()
                .expect("Failed to initialize agent registry"),
        )
        .setup(move |app| {
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(log_level)
                    .build(),
            )?;

            log::info!(
                "Logger initialized with level={} (RUST_LOG={})",
                log_level,
                std::env::var("RUST_LOG").unwrap_or_else(|_| "<unset>".to_string())
            );

            let config_path = config::init_config_if_missing().map_err(std::io::Error::other)?;
            let upgrade_result = config::upgrade_config_file().map_err(std::io::Error::other)?;
            log::info!("Config file ready at {}", config_path.display());
            if upgrade_result.upgraded {
                log::info!(
                    "Config file normalized and missing fields were filled at {}",
                    upgrade_result.config_path
                );
            }

            // Ensure the default "main" agent profile exists on disk
            if let Err(err) = config::ensure_default_agent_profile() {
                log::warn!("Failed to ensure default agent profile: {}", err);
            }

            let config_event_state =
                app.state::<std::sync::Arc<commands::config::ConfigEventState>>();
            commands::config::start_config_watcher(
                app.handle().clone(),
                config_event_state.inner().clone(),
            )
            .map_err(std::io::Error::other)?;

            // Initialize built-in embedding providers

            std::thread::spawn(|| {
                loop {
                    let sync_result =
                        tauri::async_runtime::block_on(models::sync_remote_model_cache());
                    if let Err(err) = sync_result {
                        log::warn!("Model cache sync skipped: {}", err);
                    }
                    std::thread::sleep(std::time::Duration::from_secs(
                        models::MODEL_CACHE_SYNC_INTERVAL_SECONDS,
                    ));
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::chat::chat_stream,
            commands::chat::cancel_chat_stream,
            commands::chat::list_conversations,
            commands::chat::load_conversation,
            commands::chat::load_conversation_dynamic_tools,
            commands::chat::replace_conversation_dynamic_tools,
            commands::chat::upsert_conversation_dynamic_tool,
            commands::chat::remove_conversation_dynamic_tool,
            commands::chat::rename_conversation,
            commands::chat::delete_conversation,
            commands::agents::list_agents,
            commands::agents::create_agent,
            commands::agents::delete_agent,
            commands::agents::get_agent_config,
            commands::config::get_runtime_config,
            commands::config::get_config_file_path,
            commands::config::upgrade_config_file,
            commands::config::get_settings_snapshot,
            commands::config::get_settings_ui_snapshot,
            commands::config::apply_settings_patch,
            commands::tools::get_tool_manager_snapshot,
            commands::tools::get_plugin_manager_snapshot,
            commands::tools::get_plugin_settings_snapshot,
            commands::models::get_model_catalog,
            commands::models::force_sync_model_cache,
            commands::devtools::open_devtools,
            commands::sub_agents::cancel_sub_agent,
            commands::sub_agents::list_sub_agents,
            commands::street::get_street_items,
            commands::street::dismiss_street_item,
            commands::memory::list_memories,
            commands::memory::get_memory,
            commands::memory::search_memories,
            commands::memory::delete_memory,
            commands::memory::open_memory_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
