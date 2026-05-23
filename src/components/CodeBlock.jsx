import { useState } from 'react';

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
      <div className="flex items-center justify-between bg-[#131314] px-4 py-2 text-xs font-medium text-slate-400">
        <span className="capitalize">{language}</span>
        <div className="flex items-center gap-3">
          <button
            onClick={handleDownload}
            className="flex items-center gap-1 hover:text-slate-200 transition"
            title="下载代码"
          >
            <svg xmlns="http://www.w3.org/2000/svg" className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
            </svg>
            <span>下载</span>
          </button>
          <button
            onClick={handleCopy}
            className="flex items-center gap-1 hover:text-slate-200 transition"
            title="复制代码"
          >
            {copied ? (
              <>
                <svg xmlns="http://www.w3.org/2000/svg" className="h-4 w-4 text-emerald-400" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                </svg>
                <span className="text-emerald-400">已复制!</span>
              </>
            ) : (
              <>
                <svg xmlns="http://www.w3.org/2000/svg" className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 5H6a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2v-1M8 5a2 2 0 002 2h2a2 2 0 002-2M8 5a2 2 0 012-2h2a2 2 0 012 2m0 0h2a2 2 0 012 2v3m2 4H10m0 0l3-3m-3 3l3 3" />
                </svg>
                <span>复制</span>
              </>
            )}
          </button>
        </div>
      </div>
      {/* Code body */}
      <div className="overflow-x-auto p-4 font-mono text-sm leading-relaxed scrollbar-thin">
        <pre><code>{code}</code></pre>
      </div>
    </div>
  );
}
