import { useEffect, useRef, useState } from 'react';
import { AlertCircle, Image, Mic, Paperclip, Send, SlidersHorizontal, Square, X } from 'lucide-react';

interface ComposerAttachment {
  name: string;
  type: string;
}

interface ChatComposerProps {
  input: string;
  onInputChange: (value: string) => void;
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
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const advancedTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const [advancedPanelOpen, setAdvancedPanelOpen] = useState(false);

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
        <div className="relative flex flex-col rounded-3xl border border-[#2d2f31] bg-[#1e1f20] px-4 py-3 shadow-md transition duration-200 focus-within:border-[#3c4043] focus-within:ring-1 focus-within:ring-[#3c4043]/50">
          {attachment && (
            <div className="mb-2 flex items-center gap-2 self-start rounded-xl border border-[#2d2f31] bg-[#131314] p-1.5 pr-2.5">
              <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-red-500/10 text-red-400">
                <Image className="h-5 w-5" />
              </div>
              <span className="text-xs font-medium text-slate-300">{attachment.name}</span>
              <button
                onClick={onRemoveAttachment}
                className="ml-2 rounded-full p-0.5 text-slate-400 hover:bg-[#2d2f31] hover:text-slate-200"
              >
                <X className="h-3.5 w-3.5" />
              </button>
            </div>
          )}

          {advancedPanelOpen && (
            <div className="mb-2 rounded-2xl border border-[#2d2f31] bg-[#171819] p-3">
              <div className="mb-2 flex items-center justify-between gap-2">
                <span className="text-xs font-medium text-slate-300">高级请求参数 (JSON)</span>
                <button
                  onClick={() => setAdvancedPanelOpen(false)}
                  className="rounded-full p-1 text-slate-500 transition hover:bg-[#2d2f31] hover:text-slate-200"
                  title="收起高级参数"
                >
                  <X className="h-3.5 w-3.5" />
                </button>
              </div>
              <textarea
                ref={advancedTextareaRef}
                value={advancedRequestOptionsInput}
                onChange={(event) => onAdvancedRequestOptionsInputChange(event.target.value)}
                rows={4}
                data-native-context-menu="true"
                placeholder='{"serviceTier":"flex","include":["reasoning.encrypted_content"]}'
                className="scrollbar-thin max-h-[220px] w-full resize-none rounded-xl border border-[#2d2f31] bg-[#111214] px-2.5 py-2 font-mono text-xs leading-relaxed text-slate-200 placeholder-slate-600 outline-none transition focus:border-[#3c4043] focus:bg-[#15171a] select-text"
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
              className="rounded-full p-2 text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200"
              title="上传文件/图片"
            >
              <Paperclip className="h-5.5 w-5.5" />
            </button>

            <textarea
              ref={textareaRef}
              value={input}
              onChange={(event) => onInputChange(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && !event.shiftKey) {
                  event.preventDefault();
                  handleSubmit();
                }
              }}
              placeholder="问问 AgentJax"
              rows={1}
              data-native-context-menu="true"
              className="scrollbar-thin max-h-[180px] flex-1 resize-none bg-transparent py-1.5 text-sm text-slate-200 placeholder-slate-500 focus:outline-none select-text"
            />

            <div className="flex items-center gap-2">
              <button
                onClick={() => setAdvancedPanelOpen((open) => !open)}
                className={`rounded-full p-2 transition ${
                  advancedPanelOpen
                    ? 'bg-cyan-400/10 text-cyan-200'
                    : 'text-slate-400 hover:bg-[#2d2f31] hover:text-slate-200'
                }`}
                title="高级请求参数"
              >
                <SlidersHorizontal className="h-5 w-5" />
              </button>

              <button
                className="rounded-full p-2 text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200"
                title="语音输入"
              >
                <Mic className="h-5.5 w-5.5" />
              </button>

              <button
                onClick={handleSubmit}
                disabled={isGenerating ? isStopping : !input.trim()}
                className={`rounded-full p-2 transition ${
                  isGenerating
                    ? 'bg-red-500/90 text-white shadow shadow-red-500/20 hover:bg-red-500'
                    : input.trim()
                      ? 'cursor-pointer bg-gradient-to-tr from-cyan-400 via-purple-500 to-pink-500 text-white shadow shadow-purple-500/20 hover:opacity-90'
                      : 'bg-transparent text-slate-600'
                }`}
                title={isGenerating ? '停止生成' : '发送消息'}
              >
                {isGenerating ? (
                  <Square className="h-4.5 w-4.5 fill-current" />
                ) : (
                  <Send className="h-4.5 w-4.5" />
                )}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
