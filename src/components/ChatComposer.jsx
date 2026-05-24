import { useEffect, useRef } from 'react';
import { Image, X, Paperclip, Mic, Send, Square } from 'lucide-react';

export default function ChatComposer({
  input,
  onInputChange,
  attachment,
  onRemoveAttachment,
  onAttachFile,
  isGenerating,
  isStopping,
  onSend,
  onStop
}) {
  const textareaRef = useRef(null);

  useEffect(() => {
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
      textareaRef.current.style.height = `${Math.min(textareaRef.current.scrollHeight, 180)}px`;
    }
  }, [input]);

  const handleSubmit = () => {
    if (isGenerating) {
      onStop();
      return;
    }

    onSend();
  };

  return (
    <div className="bg-[#131314] px-4 pb-6 pt-2 md:px-6">
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
