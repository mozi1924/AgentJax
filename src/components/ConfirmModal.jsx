import { useEffect, useRef } from 'react';
import { AlertTriangle } from 'lucide-react';

export default function ConfirmModal({
  title,
  message,
  confirmText = '确认删除',
  cancelText = '取消',
  onConfirm,
  onCancel
}) {
  const modalRef = useRef(null);

  useEffect(() => {
    // 监听 ESC 键自动关闭
    const handleKeyDown = (event) => {
      if (event.key === 'Escape') {
        onCancel();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [onCancel]);

  return (
    <div
      onClick={onCancel}
      className="animate-modal-backdrop fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-md px-4"
    >
      <div
        ref={modalRef}
        onClick={(event) => event.stopPropagation()}
        className="animate-modal-content w-full max-w-md overflow-hidden rounded-2xl border border-[#2d2f31] bg-[#1e1f20]/95 p-6 shadow-2xl shadow-black/80"
      >
        <div className="flex items-start gap-4">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-rose-500/10 text-rose-400">
            <AlertTriangle className="h-5 w-5" />
          </div>
          <div className="flex-1 space-y-1">
            <h3 className="font-semibold text-lg text-slate-100 font-sans tracking-tight">
              {title}
            </h3>
            <p className="text-sm text-slate-400 leading-relaxed">
              {message}
            </p>
          </div>
        </div>

        <div className="mt-6 flex justify-end gap-3">
          <button
            onClick={onCancel}
            className="rounded-xl px-4 py-2.5 text-sm font-medium text-slate-400 hover:text-slate-200 hover:bg-[#2d2f31]/60 transition-all duration-200 cursor-pointer"
          >
            {cancelText}
          </button>
          <button
            onClick={onConfirm}
            className="rounded-xl bg-rose-500/10 text-rose-300 border border-rose-500/20 hover:bg-rose-600 hover:text-white hover:border-transparent px-5 py-2.5 text-sm font-medium transition-all duration-200 shadow-lg shadow-rose-950/20 cursor-pointer"
          >
            {confirmText}
          </button>
        </div>
      </div>
    </div>
  );
}
