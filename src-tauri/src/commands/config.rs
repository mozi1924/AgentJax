use crate::config::{
    self, ConfigInfo, ConfigUpgradeResult, SettingsPatch, SettingsSnapshot,
};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

const CONFIG_SNAPSHOT_EVENT: &str = "config_snapshot_changed";

#[derive(Default)]
pub struct ConfigEventState {
    tracker: Mutex<RevisionTracker>,
    watcher: Mutex<Option<RecommendedWatcher>>,
}

#[derive(Default)]
struct RevisionTracker {
    last_emitted_revision: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSnapshotEvent {
    pub origin: String,
    #[serde(flatten)]
    pub snapshot: SettingsSnapshot,
}

#[tauri::command]
pub fn get_runtime_config() -> Result<ConfigInfo, String> {
    config::get_config_info()
}

#[tauri::command]
pub fn get_config_file_path() -> Result<String, String> {
    let path = config::init_config_if_missing()?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub fn upgrade_config_file() -> Result<ConfigUpgradeResult, String> {
    config::upgrade_config_file()
}

#[tauri::command]
pub fn get_settings_snapshot(
    event_state: State<'_, Arc<ConfigEventState>>,
) -> Result<SettingsSnapshot, String> {
    let snapshot = config::get_settings_snapshot()?;
    event_state.remember_revision(&snapshot.revision);
    Ok(snapshot)
}

#[tauri::command]
pub fn apply_settings_patch(
    app_handle: AppHandle,
    event_state: State<'_, Arc<ConfigEventState>>,
    patch: SettingsPatch,
) -> Result<SettingsSnapshot, String> {
    let snapshot = config::apply_settings_patch(patch)?;
    event_state.remember_revision(&snapshot.revision);
    emit_snapshot_event(&app_handle, snapshot.clone(), "internal")?;
    Ok(snapshot)
}

impl ConfigEventState {
    fn remember_revision(&self, revision: &str) {
        if let Ok(mut tracker) = self.tracker.lock() {
            tracker.last_emitted_revision = Some(revision.to_string());
        }
    }

    fn should_emit_external(&self, revision: &str) -> bool {
        match self.tracker.lock() {
            Ok(mut tracker) => {
                if tracker.last_emitted_revision.as_deref() == Some(revision) {
                    return false;
                }
                tracker.last_emitted_revision = Some(revision.to_string());
                true
            }
            Err(_) => true,
        }
    }

    fn install_watcher(&self, watcher: RecommendedWatcher) {
        if let Ok(mut slot) = self.watcher.lock() {
            *slot = Some(watcher);
        }
    }
}

pub fn start_config_watcher(
    app_handle: AppHandle,
    event_state: Arc<ConfigEventState>,
) -> Result<(), String> {
    let config_path = config::init_config_if_missing()?;
    let (tx, rx) = std::sync::mpsc::channel::<()>();

    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        match result {
            Ok(event) => {
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)) {
                    let _ = tx.send(());
                }
            }
            Err(err) => {
                log::warn!("Config watcher error: {err}");
            }
        }
    })
    .map_err(|e| format!("Failed to create config watcher: {e}"))?;

    watcher
        .watch(&config_path, RecursiveMode::NonRecursive)
        .map_err(|e| format!("Failed to watch config file {}: {e}", config_path.display()))?;
    event_state.install_watcher(watcher);

    std::thread::spawn(move || {
        while rx.recv().is_ok() {
            std::thread::sleep(Duration::from_millis(220));
            while rx.try_recv().is_ok() {}

            match config::get_settings_snapshot() {
                Ok(snapshot) => {
                    if !event_state.should_emit_external(&snapshot.revision) {
                        continue;
                    }
                    if let Err(err) = emit_snapshot_event(&app_handle, snapshot, "external") {
                        log::warn!("Failed to emit config snapshot event: {err}");
                    }
                }
                Err(err) => {
                    log::warn!("Failed to reload config snapshot after file change: {err}");
                }
            }
        }
    });

    Ok(())
}

fn emit_snapshot_event(
    app_handle: &AppHandle,
    snapshot: SettingsSnapshot,
    origin: &str,
) -> Result<(), String> {
    app_handle
        .emit(
            CONFIG_SNAPSHOT_EVENT,
            ConfigSnapshotEvent {
                origin: origin.to_string(),
                snapshot,
            },
        )
        .map_err(|e| format!("Failed to emit config snapshot event: {e}"))
}
