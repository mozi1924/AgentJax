import { useState } from 'react';
import { Check, Copy, Download } from 'lucide-react';
import { OverlayScrollArea } from './OverlayScrollArea';

interface CodeBlockProps {
  code: string;
  language: string;
}

const extensionMap: Record<string, string> = {
  python: 'py',
  javascript: 'js',
  html: 'html',
  css: 'css',
  sql: 'sql',
  bash: 'sh',
  sh: 'sh',
  json: 'json',
};

export default function CodeBlock({ code, language }: CodeBlockProps) {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    await navigator.clipboard.writeText(code);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 2000);
  };

  const handleDownload = () => {
    const element = document.createElement('a');
    const file = new Blob([code], { type: 'text/plain' });
    element.href = URL.createObjectURL(file);
    const ext = extensionMap[language.toLowerCase()] || 'txt';
    element.download = `code_snippet.${ext}`;
    document.body.appendChild(element);
    element.click();
    document.body.removeChild(element);
  };

  return (
    <div className="my-4 overflow-hidden rounded-2xl border border-[#2d2f31] bg-[#1e1f20] text-slate-200">
      <div className="flex items-center justify-between bg-[#131314] px-4 py-2 text-xs font-medium text-slate-400 select-none">
        <span className="capitalize">{language}</span>
        <div className="flex items-center gap-3">
          <button
            onClick={handleDownload}
            className="flex items-center gap-1 transition hover:text-slate-200"
            title="下载代码"
          >
            <Download className="h-4 w-4" />
            <span>下载</span>
          </button>
          <button
            onClick={handleCopy}
            className="flex items-center gap-1 transition hover:text-slate-200"
            title="复制代码"
          >
            {copied ? (
              <>
                <Check className="h-4 w-4 text-emerald-400" />
                <span className="text-emerald-400">已复制!</span>
              </>
            ) : (
              <>
                <Copy className="h-4 w-4" />
                <span>复制</span>
              </>
            )}
          </button>
        </div>
      </div>
      <OverlayScrollArea
        axis="horizontal"
        data-native-context-menu="true"
        className="p-4 font-mono text-sm leading-relaxed select-text"
      >
        <pre>
          <code>{code}</code>
        </pre>
      </OverlayScrollArea>
    </div>
  );
}
