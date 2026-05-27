import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

const isDeveloperToolsShortcut = (event: KeyboardEvent) =>
  event.key === 'F12' ||
  ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === 'i');

export function useDeveloperToolsShortcut(enabled: boolean) {
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!enabled || !isDeveloperToolsShortcut(event)) {
        return;
      }

      event.preventDefault();
      // The native command validates the persisted setting again before opening.
      void invoke('open_devtools').catch(() => {});
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [enabled]);
}
