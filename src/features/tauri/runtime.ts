import { getCurrentWindow } from '@tauri-apps/api/window';

/**
 * Frontend preview flows run outside the Tauri host, so window APIs must be
 * treated as optional instead of assumed during React effect setup.
 */
export function isTauriWindowRuntimeAvailable(): boolean {
  const runtimeWindow = window as unknown as Record<string, unknown>;
  return (
    typeof window !== 'undefined' &&
    typeof runtimeWindow.__TAURI_INTERNALS__ !== 'undefined'
  );
}

export function tryGetCurrentWindow() {
  if (!isTauriWindowRuntimeAvailable()) {
    return null;
  }

  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}
