mod commands;
mod config;
mod models;
mod openai;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .manage(commands::chat::ChatRequestRegistry::default())
    .setup(|app| {
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }

      let config_path = config::init_config_if_missing()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
      log::info!("Config file ready at {}", config_path.display());

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
      commands::chat::chat_with_responses_stream,
      commands::chat::cancel_chat_stream,
      commands::config::get_runtime_config,
      commands::config::get_config_file_path,
      commands::models::get_model_catalog,
      commands::models::force_sync_model_cache
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
