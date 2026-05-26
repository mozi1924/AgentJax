import { useEffect } from 'react';

export function useContextMenuGuard(
  canUseNativeContextMenu: (target: EventTarget | null) => boolean
) {
  useEffect(() => {
    const handleContextMenu = (event: MouseEvent) => {
      if (event.defaultPrevented) {
        return;
      }

      if (canUseNativeContextMenu(event.target)) {
        return;
      }

      event.preventDefault();
    };

    document.addEventListener('contextmenu', handleContextMenu);
    return () => {
      document.removeEventListener('contextmenu', handleContextMenu);
    };
  }, [canUseNativeContextMenu]);
}
