import { Pencil, Trash2 } from 'lucide-react';
import type { CSSProperties, RefObject } from 'react';

interface SidebarActionMenuProps {
  menuRef: RefObject<HTMLDivElement | null>;
  menuPosition: CSSProperties | null;
  onRename: () => void;
  onDelete: () => void;
}

export default function SidebarActionMenu({
  menuRef,
  menuPosition,
  onRename,
  onDelete,
}: SidebarActionMenuProps) {
  return (
    <div
      ref={menuRef}
      style={menuPosition || undefined}
      onMouseDownCapture={(event) => event.stopPropagation()}
      className="sidebar-context-menu fixed z-50 w-40 overflow-hidden rounded-2xl border border-[#3c4043] bg-[#131314]/98 p-1.5 shadow-2xl shadow-black/40 backdrop-blur-md"
    >
      <button
        onMouseDown={(event) => event.stopPropagation()}
        onClick={onRename}
        className="flex w-full items-center gap-2 rounded-xl px-3 py-2 text-left text-sm text-slate-200 transition hover:bg-[#2d2f31]"
      >
        <Pencil className="h-4 w-4" />
        <span>重命名</span>
      </button>
      <div className="my-1 border-t border-[#2d2f31]" />
      <button
        onMouseDown={(event) => event.stopPropagation()}
        onClick={onDelete}
        className="flex w-full items-center gap-2 rounded-xl px-3 py-2 text-left text-sm text-rose-300 transition hover:bg-[#2d2f31]"
      >
        <Trash2 className="h-4 w-4" />
        <span>删除</span>
      </button>
    </div>
  );
}
