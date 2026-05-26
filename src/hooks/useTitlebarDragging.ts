import { useEffect } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';

export function useTitlebarDragging(titlebarRef: React.RefObject<HTMLElement | null>) {
  useEffect(() => {
    const titlebar = titlebarRef.current;
    if (!titlebar) {
      return undefined;
    }

    const appWindow = getCurrentWindow();
    const handleMouseDown = async (event: MouseEvent) => {
      if (event.buttons !== 1) return;
      if ((event.target as HTMLElement | null)?.closest('[data-no-drag="true"]')) return;
      await appWindow.startDragging();
    };

    titlebar.addEventListener('mousedown', handleMouseDown);
    return () => {
      titlebar.removeEventListener('mousedown', handleMouseDown);
    };
  }, []);
}

