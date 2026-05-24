import { useState } from 'react';
import { Download, Check, Copy } from 'lucide-react';

export default function CodeBlock({ code, language }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (err) {
      console.error('Failed to copy text: ', err);
    }
  };

  const handleDownload = () => {
    const element = document.createElement("a");
    const file = new Blob([code], { type: 'text/plain' });
    element.href = URL.createObjectURL(file);
    const extensions = {
      python: 'py',
      javascript: 'js',
      html: 'html',
      css: 'css',
      sql: 'sql',
      bash: 'sh',
      sh: 'sh',
      json: 'json'
    };
    const ext = extensions[language.toLowerCase()] || 'txt';
    element.download = `code_snippet.${ext}`;
    document.body.appendChild(element);
    element.click();
    document.body.removeChild(element);
  };

  return (
    <div className="my-4 overflow-hidden rounded-2xl border border-[#2d2f31] bg-[#1e1f20] text-slate-200">
      {/* Header bar */}
      <div className="flex items-center justify-between bg-[#131314] px-4 py-2 text-xs font-medium text-slate-400 select-none">
        <span className="capitalize">{language}</span>
        <div className="flex items-center gap-3">
          <button
            onClick={handleDownload}
            className="flex items-center gap-1 hover:text-slate-200 transition"
            title="下载代码"
          >
            <Download className="h-4 w-4" />
            <span>下载</span>
          </button>
          <button
            onClick={handleCopy}
            className="flex items-center gap-1 hover:text-slate-200 transition"
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
      {/* Code body */}
      <div
        data-native-context-menu="true"
        className="overflow-x-auto p-4 font-mono text-sm leading-relaxed scrollbar-thin select-text"
      >
        <pre><code>{code}</code></pre>
      </div>
    </div>
  );
}
