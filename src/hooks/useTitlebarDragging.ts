import { useEffect } from 'react';
import { tryGetCurrentWindow } from '../features/tauri/runtime';

export function useTitlebarDragging(titlebarRef: React.RefObject<HTMLElement | null>) {
  useEffect(() => {
    const titlebar = titlebarRef.current;
    if (!titlebar) {
      return undefined;
    }

    const appWindow = tryGetCurrentWindow();
    if (!appWindow) {
      return undefined;
    }

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
