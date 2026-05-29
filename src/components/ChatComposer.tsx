import { useEffect, useRef, useState } from 'react';
import { AlertCircle, Image, Mic, Paperclip, Send, SlidersHorizontal, Square, X } from 'lucide-react';
import { useI18n } from '../features/i18n';
import { OverlayTextarea } from './OverlayScrollArea';

interface ComposerAttachment {
  name: string;
  type: string;
}

interface ChatComposerProps {
  input: string;
  onInputChange: (value: string) => void;
  showAdvancedRequestOptionsButton: boolean;
  advancedRequestOptionsInput: string;
  onAdvancedRequestOptionsInputChange: (value: string) => void;
  advancedRequestOptionsError?: string | null;
  attachment: ComposerAttachment | null;
  onRemoveAttachment: () => void;
  onAttachFile: () => void;
  isGenerating: boolean;
  isStopping: boolean;
  onSend: () => void;
  onStop: () => void;
}

export default function ChatComposer({
  input,
  onInputChange,
  showAdvancedRequestOptionsButton,
  advancedRequestOptionsInput,
  onAdvancedRequestOptionsInputChange,
  advancedRequestOptionsError,
  attachment,
  onRemoveAttachment,
  onAttachFile,
  isGenerating,
  isStopping,
  onSend,
  onStop,
}: ChatComposerProps) {
  const { t } = useI18n();
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const advancedTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const [advancedPanelOpen, setAdvancedPanelOpen] = useState(false);

  useEffect(() => {
    if (showAdvancedRequestOptionsButton) {
      return;
    }
    setAdvancedPanelOpen(false);
  }, [showAdvancedRequestOptionsButton]);

  useEffect(() => {
    if (!textareaRef.current) return;
    textareaRef.current.style.height = 'auto';
    textareaRef.current.style.height = `${Math.min(textareaRef.current.scrollHeight, 180)}px`;
  }, [input]);

  useEffect(() => {
    if (!advancedTextareaRef.current) return;
    advancedTextareaRef.current.style.height = 'auto';
    advancedTextareaRef.current.style.height = `${Math.min(advancedTextareaRef.current.scrollHeight, 220)}px`;
  }, [advancedRequestOptionsInput, advancedPanelOpen]);

  const handleSubmit = () => {
    if (isGenerating) {
      onStop();
      return;
    }

    onSend();
  };

  return (
    <div className="relative bg-transparent px-4 pt-2 pb-6 md:px-8 lg:px-12">
      <div className="mx-auto flex max-w-3xl flex-col">
        <div className="relative flex flex-col rounded-2xl border border-[#272a2e] bg-[#161719] px-4 py-3 shadow-[0_8px_32px_rgba(0,0,0,0.3)] transition duration-200 focus-within:border-[#3a3e45] focus-within:ring-1 focus-within:ring-[#3a3e45]/50">
          {attachment && (
            <div className="mb-2 flex items-center gap-2 self-start rounded-xl border border-[#24272b] bg-[#0d0e0f] p-1.5 pr-2.5">
              <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-slate-500/10 text-slate-400">
                <Image className="h-4.5 w-4.5" />
              </div>
              <span className="text-xs font-medium text-slate-300">{attachment.name}</span>
              <button
                onClick={onRemoveAttachment}
                className="ml-2 rounded-full p-0.5 text-slate-500 hover:bg-[#222427] hover:text-slate-200"
              >
                <X className="h-3.5 w-3.5" />
              </button>
            </div>
          )}

          {showAdvancedRequestOptionsButton && advancedPanelOpen && (
            <div className="mb-2 rounded-xl border border-[#25282d] bg-[#111214] p-3">
              <div className="mb-2 flex items-center justify-between gap-2">
                <span className="text-xs font-medium text-slate-400">{t('composer.advanced_request')}</span>
                <button
                  onClick={() => setAdvancedPanelOpen(false)}
                  className="rounded-full p-1 text-slate-500 transition hover:bg-[#222427] hover:text-slate-200"
                  title={t('composer.hide_advanced')}
                >
                  <X className="h-3.5 w-3.5" />
                </button>
              </div>
              <OverlayTextarea
                ref={advancedTextareaRef}
                value={advancedRequestOptionsInput}
                onChange={(event) => onAdvancedRequestOptionsInputChange(event.target.value)}
                rows={4}
                data-native-context-menu="true"
                placeholder='{"serviceTier":"flex","include":["reasoning.encrypted_content"]}'
                containerClassName="w-full"
                className="max-h-[220px] w-full resize-none rounded-xl border border-[#25282d] bg-[#0d0e0f] px-2.5 py-2 font-mono text-xs leading-relaxed text-slate-300 placeholder-slate-600 outline-none transition focus:border-[#383d45] focus:bg-[#111214] select-text"
              />
              {advancedRequestOptionsError && (
                <div className="mt-2 inline-flex items-start gap-1.5 rounded-lg border border-rose-500/20 bg-rose-500/10 px-2 py-1 text-[11px] text-rose-200">
                  <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                  <span>{advancedRequestOptionsError}</span>
                </div>
              )}
            </div>
          )}

          <div className="flex items-center gap-3">
            <button
              onClick={onAttachFile}
              className="rounded-full p-2 text-slate-400 transition hover:bg-[#222427] hover:text-slate-200"
              title={t('composer.upload_file')}
            >
              <Paperclip className="h-5 w-5" />
            </button>

            <OverlayTextarea
              ref={textareaRef}
              value={input}
              onChange={(event) => onInputChange(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && !event.shiftKey) {
                  event.preventDefault();
                  handleSubmit();
                }
              }}
              placeholder={t('composer.placeholder')}
              rows={1}
              data-native-context-menu="true"
              containerClassName="flex-1"
              className="max-h-[180px] w-full resize-none bg-transparent py-1.5 text-sm text-slate-200 placeholder-slate-500 focus:outline-none select-text"
            />

            <div className="flex items-center gap-2">
              {showAdvancedRequestOptionsButton && (
                <button
                  onClick={() => setAdvancedPanelOpen((open) => !open)}
                  className={`rounded-full p-2 transition ${
                    advancedPanelOpen
                      ? 'bg-slate-700/30 text-slate-200'
                      : 'text-slate-400 hover:bg-[#222427] hover:text-slate-200'
                  }`}
                  title={t('composer.advanced_request')}
                >
                  <SlidersHorizontal className="h-4.5 w-4.5" />
                </button>
              )}

              <button
                className="rounded-full p-2 text-slate-400 transition hover:bg-[#222427] hover:text-slate-200"
                title={t('composer.audio_input')}
              >
                <Mic className="h-5 w-5" />
              </button>

              <button
                onClick={handleSubmit}
                disabled={isGenerating ? isStopping : !input.trim()}
                className={`rounded-full p-2 transition-all duration-200 ${
                  isGenerating
                    ? 'bg-rose-600 hover:bg-rose-500 text-white shadow shadow-rose-900/20 active:scale-95'
                    : input.trim()
                      ? 'cursor-pointer bg-slate-100 hover:bg-white text-slate-950 shadow-sm active:scale-95'
                      : 'bg-transparent text-slate-600'
                }`}
                title={isGenerating ? t('composer.stop_generating') : t('composer.send_message')}
              >
                {isGenerating ? (
                  <Square className="h-4 w-4 fill-current" />
                ) : (
                  <Send className="h-4 w-4" />
                )}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
