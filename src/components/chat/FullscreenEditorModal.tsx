import { useEffect, useRef, useState } from 'react';
import { AlignLeft, Code, Send, X } from 'lucide-react';
import { useI18n } from '../../features/i18n';
import { OverlayTextarea } from '../OverlayScrollArea';

interface FullscreenEditorModalProps {
  isOpen: boolean;
  value: string;
  onClose: () => void;
  onSave: (text: string) => void;
  onSend: (text: string) => void;
  isGenerating: boolean;
  isStopping: boolean;
}

export default function FullscreenEditorModal({
  isOpen,
  value,
  onClose,
  onSave,
  onSend,
  isGenerating,
  isStopping,
}: FullscreenEditorModalProps) {
  const { t } = useI18n();
  const [text, setText] = useState(value);
  const [isMonospace, setIsMonospace] = useState(true);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  // Sync internal state when opened or value changed externally
  useEffect(() => {
    if (isOpen) {
      setText(value);
      // Auto focus the textarea after render
      setTimeout(() => {
        if (textareaRef.current) {
          textareaRef.current.focus();
          // Put cursor at the end
          textareaRef.current.selectionStart = textareaRef.current.value.length;
          textareaRef.current.selectionEnd = textareaRef.current.value.length;
        }
      }, 50);
    }
  }, [isOpen, value]);

  // Handle ESC and Ctrl/Cmd+Enter keyboard shortcuts
  useEffect(() => {
    if (!isOpen) return;

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        onClose();
      } else if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
        event.preventDefault();
        handleSend();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [isOpen, text, onSave, onSend, isGenerating, isStopping]);

  if (!isOpen) return null;

  // Calculate live statistics
  const charCount = text.length;
  const wordCount = text.trim() ? text.trim().split(/\s+/).filter(Boolean).length : 0;
  const lineCount = text ? text.split('\n').length : 0;

  const handleApply = () => {
    onSave(text);
    onClose();
  };

  const handleSend = () => {
    if (isGenerating) return;
    onSend(text);
    onClose();
  };

  return (
    <div
      onClick={onClose}
      className="animate-modal-backdrop fixed inset-0 z-[100] flex items-center justify-center bg-black/60 px-4 py-6 backdrop-blur-md"
    >
      <div
        onClick={(event) => event.stopPropagation()}
        className="animate-modal-content flex h-[85vh] w-[92vw] max-w-5xl flex-col overflow-hidden rounded-2xl border border-[#2b2b2d] bg-[#161719]/95 shadow-2xl shadow-black/80"
      >
        {/* Header */}
        <header className="flex items-center justify-between border-b border-[#272a2e] px-6 py-4">
          <div className="flex items-center gap-3">
            <h3 className="font-sans text-base font-bold text-slate-100">
              {t('composer.fullscreen_editor.title')}
            </h3>
            <div className="flex items-center gap-1 rounded-lg bg-[#222427] p-0.5">
              <button
                onClick={() => setIsMonospace(true)}
                className={`flex h-6 w-8 items-center justify-center rounded-md transition ${
                  isMonospace ? 'bg-slate-700/60 text-slate-200' : 'text-slate-500 hover:text-slate-300'
                }`}
                title="Monospace Font (Code/Config)"
              >
                <Code className="h-4 w-4" />
              </button>
              <button
                onClick={() => setIsMonospace(false)}
                className={`flex h-6 w-8 items-center justify-center rounded-md transition ${
                  !isMonospace ? 'bg-slate-700/60 text-slate-200' : 'text-slate-500 hover:text-slate-300'
                }`}
                title="Sans-serif Font (Normal Text)"
              >
                <AlignLeft className="h-4 w-4" />
              </button>
            </div>
          </div>

          <button
            onClick={onClose}
            className="flex h-7 w-7 items-center justify-center rounded-lg text-slate-400 hover:bg-[#222427] hover:text-slate-100 transition"
            title={t('settings.modal.close')}
          >
            <X className="h-4.5 w-4.5" />
          </button>
        </header>

        {/* Content Area - Spacious custom scrollbar textarea */}
        <div className="relative flex-1 bg-[#0d0e0f] p-4 min-h-0">
          <OverlayTextarea
            ref={(node) => {
              textareaRef.current = node;
            }}
            value={text}
            onChange={(event) => setText(event.target.value)}
            placeholder={t('composer.fullscreen_editor.placeholder')}
            data-native-context-menu="true"
            containerClassName="w-full h-full"
            className={`w-full h-full resize-none bg-transparent px-4 py-2 text-sm leading-relaxed text-slate-200 placeholder-slate-600 focus:outline-none select-text ${
              isMonospace ? 'font-mono text-xs' : 'font-sans'
            }`}
          />
        </div>

        {/* Footer with stats & controls */}
        <footer className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 border-t border-[#272a2e] bg-[#161719] px-6 py-4">
          {/* Real-time counters */}
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-slate-400">
            <span className="font-mono">
              {t('composer.fullscreen_editor.char_count', { count: String(charCount) })}
            </span>
            <span className="h-3 w-px bg-slate-800" />
            <span>
              {t('composer.fullscreen_editor.word_count', { count: String(wordCount) })}
            </span>
            <span className="h-3 w-px bg-slate-800" />
            <span>
              {lineCount} {lineCount > 1 ? 'lines' : 'line'}
            </span>
          </div>

          {/* Action buttons */}
          <div className="flex items-center justify-end gap-3">
            <button
              onClick={onClose}
              className="cursor-pointer rounded-xl px-4 py-2.5 text-sm font-medium text-slate-400 transition-all duration-200 hover:bg-[#222427] hover:text-slate-200"
            >
              {t('composer.fullscreen_editor.cancel')}
            </button>
            <button
              onClick={handleApply}
              className="cursor-pointer rounded-xl border border-slate-700 bg-slate-800/40 px-4 py-2.5 text-sm font-medium text-slate-300 transition-all duration-200 hover:bg-slate-850 hover:text-white"
            >
              {t('composer.fullscreen_editor.save')}
            </button>
            <button
              onClick={handleSend}
              disabled={isGenerating ? isStopping : !text.trim()}
              className={`flex cursor-pointer items-center gap-1.5 rounded-xl px-5 py-2.5 text-sm font-medium transition-all duration-200 active:scale-95 ${
                text.trim() && !isGenerating
                  ? 'bg-slate-100 text-slate-950 hover:bg-white shadow-sm'
                  : 'bg-transparent text-slate-600 pointer-events-none'
              }`}
              title="Ctrl/Cmd + Enter"
            >
              <Send className="h-3.5 w-3.5" />
              <span>{t('composer.fullscreen_editor.send')}</span>
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}
