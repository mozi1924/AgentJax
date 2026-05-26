#![recursion_limit = "512"]

mod agentjax_home;
mod commands;
mod config;
mod conversation_store;
mod conversation_store_utils;
mod message_phase;
pub mod mcp;
mod models;
mod providers;
pub mod runtime;
pub mod tools;

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

            let config_path =
                config::init_config_if_missing().map_err(|e| std::io::Error::other(e))?;
            let upgrade_result =
                config::upgrade_config_file().map_err(|e| std::io::Error::other(e))?;
            log::info!("Config file ready at {}", config_path.display());
            if upgrade_result.upgraded {
                log::info!(
                    "Config file normalized and missing fields were filled at {}",
                    upgrade_result.config_path
                );
            }
            let config_event_state =
                app.state::<std::sync::Arc<commands::config::ConfigEventState>>();
            commands::config::start_config_watcher(
                app.handle().clone(),
                config_event_state.inner().clone(),
            )
            .map_err(std::io::Error::other)?;

            std::thread::spawn(|| loop {
                let sync_result = tauri::async_runtime::block_on(models::sync_remote_model_cache());
                if let Err(err) = sync_result {
                    log::warn!("Model cache sync skipped: {}", err);
                }
                std::thread::sleep(std::time::Duration::from_secs(
                    models::MODEL_CACHE_SYNC_INTERVAL_SECONDS,
                ));
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::chat::chat_stream,
            commands::chat::cancel_chat_stream,
            commands::chat::list_conversations,
            commands::chat::load_conversation,
            commands::chat::rename_conversation,
            commands::chat::delete_conversation,
            commands::config::get_runtime_config,
            commands::config::get_config_file_path,
            commands::config::upgrade_config_file,
            commands::config::get_settings_snapshot,
            commands::config::get_settings_ui_snapshot,
            commands::config::apply_settings_patch,
            commands::models::get_model_catalog,
            commands::models::force_sync_model_cache,
            commands::devtools::open_devtools
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
