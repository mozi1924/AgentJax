import { useState } from 'react';
import type { MouseEvent } from 'react';
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Copy,
  Loader2,
} from 'lucide-react';
import type { ToolCall } from '../../features/conversations/types';

interface ToolCallWidgetProps {
  toolCall: ToolCall;
}

const statusStyles: Record<string, string> = {
  started: 'text-amber-400 border-amber-500/20 bg-amber-500/5',
  arguments_done: 'text-cyan-400 border-cyan-500/20 bg-cyan-500/5',
  executed: 'text-emerald-400 border-emerald-500/20 bg-emerald-500/5',
  failed: 'text-rose-400 border-rose-500/20 bg-rose-500/5',
};

const statusText: Record<string, string> = {
  started: '正在构建参数...',
  arguments_done: '正在执行中...',
  executed: '执行成功',
  failed: '执行失败',
};

const formatValue = (val: unknown): string => {
  if (!val) return '';
  try {
    const parsed = typeof val === 'string' ? JSON.parse(val) : val;
    return JSON.stringify(parsed, null, 2);
  } catch {
    return String(val);
  }
};

const resolveToolMeta = (name: string) => {
  let displayName = name;
  let originLabel = '内置工具';

  if (name.startsWith('mcp__')) {
    const parts = name.split('__');
    if (parts.length >= 3) {
      const serverId = parts[1];
      const toolName = parts.slice(2).join('__');
      displayName = toolName;
      originLabel = `MCP: ${serverId}`;
    }
  }

  return { displayName, originLabel };
};

const isToolCallFailed = (toolCall: ToolCall) => {
  if (toolCall.status === 'failed') return true;
  if (!toolCall.output) return false;
  try {
    const parsed =
      typeof toolCall.output === 'string'
        ? JSON.parse(toolCall.output)
        : toolCall.output;
    return parsed?.ok === false || !!parsed?.error;
  } catch {
    return String(toolCall.output).toLowerCase().includes('failed');
  }
};

export default function ToolCallWidget({ toolCall }: ToolCallWidgetProps) {
  const [expanded, setExpanded] = useState(false);
  const [copiedArgs, setCopiedArgs] = useState(false);
  const [copiedOutput, setCopiedOutput] = useState(false);

  const { displayName, originLabel } = resolveToolMeta(toolCall.name || '');
  const failed = isToolCallFailed(toolCall);

  const handleCopyArgs = async (event: MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation();
    await navigator.clipboard.writeText(formatValue(toolCall.arguments) || '{}');
    setCopiedArgs(true);
    window.setTimeout(() => setCopiedArgs(false), 2000);
  };

  const handleCopyOutput = async (event: MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation();
    await navigator.clipboard.writeText(formatValue(toolCall.output));
    setCopiedOutput(true);
    window.setTimeout(() => setCopiedOutput(false), 2000);
  };

  return (
    <div
      className={`mb-2 rounded-xl border px-4 py-2 text-xs leading-relaxed transition-all duration-300 backdrop-blur-md ${
        failed
          ? 'text-rose-400 border-rose-500/20 bg-rose-500/5'
          : statusStyles[toolCall.status] ||
            'text-slate-400 border-slate-500/20 bg-slate-500/5'
      }`}
    >
      <div
        className="flex cursor-pointer items-center justify-between"
        onClick={() => setExpanded((current) => !current)}
      >
        <div className="flex items-center gap-2 font-mono">
          {toolCall.status === 'executed' && !failed ? (
            <CheckCircle2 className="h-4 w-4 shrink-0 text-emerald-400" />
          ) : failed ? (
            <AlertTriangle className="h-4 w-4 shrink-0 text-rose-400" />
          ) : (
            <Loader2 className="h-4 w-4 shrink-0 animate-spin text-cyan-400" />
          )}
          <span className="mr-0.5 rounded bg-slate-500/10 px-1.5 py-0.5 font-sans text-[10px] opacity-75">
            {originLabel}
          </span>
          <span className="font-semibold text-slate-200">{displayName}</span>
          <span className="opacity-75">
            ({statusText[toolCall.status] || '等待中'})
          </span>
          {toolCall.durationMs !== undefined && toolCall.durationMs !== null && (
            <span className="ml-1 text-[10px] opacity-50">[{toolCall.durationMs}ms]</span>
          )}
        </div>
        <div className="flex items-center gap-1.5 opacity-70 transition hover:opacity-100">
          <span className="text-[10px]">详情</span>
          {expanded ? (
            <ChevronUp className="h-3.5 w-3.5" />
          ) : (
            <ChevronDown className="h-3.5 w-3.5" />
          )}
        </div>
      </div>

      {expanded && (
        <div className="mt-2.5 space-y-2 border-t border-slate-500/10 pt-2.5 transition-all">
          <div>
            <div className="mb-1 flex items-center justify-between select-none">
              <span className="font-semibold opacity-70">输入参数：</span>
              <button
                onClick={handleCopyArgs}
                className="flex cursor-pointer items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium text-cyan-300 opacity-60 transition hover:bg-slate-500/10 hover:opacity-100"
                title="复制输入参数"
              >
                {copiedArgs ? (
                  <>
                    <CheckCircle2 className="h-3 w-3 text-emerald-400" />
                    <span className="text-emerald-400">已复制</span>
                  </>
                ) : (
                  <>
                    <Copy className="h-3 w-3" />
                    <span>复制</span>
                  </>
                )}
              </button>
            </div>
            <pre className="scrollbar-thin max-h-40 overflow-x-auto whitespace-pre-wrap rounded-lg bg-[#1e1f20]/60 p-2 font-mono text-[10px] text-cyan-300 select-text">
              {formatValue(toolCall.arguments) || '{}'}
            </pre>
          </div>
          {toolCall.output && (
            <div>
              <div className="mb-1 flex items-center justify-between select-none">
                <span className="font-semibold opacity-70">输出结果：</span>
                <button
                  onClick={handleCopyOutput}
                  className="flex cursor-pointer items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium text-emerald-300 opacity-60 transition hover:bg-slate-500/10 hover:opacity-100"
                  title="复制输出结果"
                >
                  {copiedOutput ? (
                    <>
                      <CheckCircle2 className="h-3 w-3 text-emerald-400" />
                      <span className="text-emerald-400">已复制</span>
                    </>
                  ) : (
                    <>
                      <Copy className="h-3 w-3" />
                      <span>复制</span>
                    </>
                  )}
                </button>
              </div>
              <pre
                className={`scrollbar-thin max-h-40 overflow-x-auto whitespace-pre-wrap rounded-lg p-2 font-mono text-[10px] select-text ${
                  failed
                    ? 'bg-rose-950/20 text-rose-300'
                    : 'bg-[#131314]/80 text-emerald-300'
                }`}
              >
                {formatValue(toolCall.output)}
              </pre>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
