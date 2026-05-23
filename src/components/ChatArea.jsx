import { useEffect, useRef } from 'react';
import CodeBlock from './CodeBlock';

export default function ChatArea({
  messages,
  isGenerating,
  activeChatTitle
}) {
  const messagesEndRef = useRef(null);

  // Auto-scroll to bottom
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, isGenerating]);

  // Helper parser for inline rich styles (bold, code, alerts)
  const parseInlineElements = (text) => {
    // Regex for inline code: `code`
    // Regex for bold text: **text**
    const parts = text.split(/(\*\*.*?\*\*|`.*?`)/g);
    return parts.map((part, index) => {
      if (part.startsWith('**') && part.endsWith('**')) {
        return <strong key={index} className="font-semibold text-slate-100">{part.slice(2, -2)}</strong>;
      } else if (part.startsWith('`') && part.endsWith('`')) {
        return (
          <code key={index} className="mx-0.5 rounded bg-[#2d2f31] px-1.5 py-0.5 font-mono text-xs text-cyan-300">
            {part.slice(1, -1)}
          </code>
        );
      }
      return part;
    });
  };

  // Parses markdown paragraphs, lists, headers, warnings, and tables
  const parseParagraphs = (rawText) => {
    const lines = rawText.split('\n');
    let elements = [];
    let listItems = [];
    let inList = false;
    let listType = 'unordered'; // 'unordered' or 'ordered'

    const commitList = (key) => {
      if (listItems.length > 0) {
        if (listType === 'ordered') {
          elements.push(
            <ol key={`ol-${key}`} className="list-decimal pl-6 space-y-1 my-2">
              {listItems}
            </ol>
          );
        } else {
          elements.push(
            <ul key={`ul-${key}`} className="list-disc pl-6 space-y-1 my-2">
              {listItems}
            </ul>
          );
        }
        listItems = [];
        inList = false;
      }
    };

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];

      // Handle Markdown tables
      if (line.trim().startsWith('|') && lines[i+1]?.trim().startsWith('|')) {
        commitList(i);
        
        // Parse simple tables
        const tableLines = [];
        let j = i;
        while (j < lines.length && lines[j].trim().startsWith('|')) {
          tableLines.push(lines[j]);
          j++;
        }
        i = j - 1; // skip forward

        const headerLine = tableLines[0];
        const dividerLine = tableLines[1];
        const rowLines = tableLines.slice(2);

        const parseTableRow = (rowText) => {
          return rowText
            .split('|')
            .slice(1, -1)
            .map(cell => cell.trim());
        };

        const headers = parseTableRow(headerLine);
        const rows = rowLines.map(parseTableRow);

        elements.push(
          <div key={`table-${i}`} className="my-4 overflow-x-auto rounded-xl border border-[#2d2f31]">
            <table className="min-w-full divide-y divide-[#2d2f31] text-xs">
              <thead className="bg-[#131314]">
                <tr>
                  {headers.map((header, hIdx) => (
                    <th key={hIdx} className="px-4 py-3 text-left font-semibold text-slate-300">
                      {header}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody className="divide-y divide-[#2d2f31] bg-[#1e1f20]/30">
                {rows.map((row, rIdx) => (
                  <tr key={rIdx} className="hover:bg-[#2d2f31]/20">
                    {row.map((cell, cIdx) => (
                      <td key={cIdx} className="px-4 py-2.5 text-slate-300">
                        {parseInlineElements(cell)}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        );
        continue;
      }

      // Handle Headers
      if (line.startsWith('### ')) {
        commitList(i);
        elements.push(
          <h3 key={i} className="text-base font-semibold text-slate-100 mt-5 mb-2">
            {parseInlineElements(line.slice(4))}
          </h3>
        );
      } else if (line.startsWith('#### ')) {
        commitList(i);
        elements.push(
          <h4 key={i} className="text-sm font-semibold text-slate-200 mt-4 mb-1">
            {parseInlineElements(line.slice(5))}
          </h4>
        );
      } else if (line.startsWith('## ')) {
        commitList(i);
        elements.push(
          <h2 key={i} className="text-lg font-semibold text-slate-100 mt-6 mb-3 border-b border-[#2d2f31] pb-1.5">
            {parseInlineElements(line.slice(3))}
          </h2>
        );
      } else if (line.startsWith('---')) {
        commitList(i);
        elements.push(<hr key={i} className="my-5 border-t border-[#2d2f31]" />);
      } else if (line.startsWith('***')) {
        commitList(i);
        elements.push(<hr key={i} className="my-5 border-t border-[#2d2f31]" />);
      }
      
      // Handle Alerts (e.g. > [!NOTE], > [!WARNING], > [!TIP])
      else if (line.startsWith('> [!')) {
        commitList(i);
        const match = line.match(/^> \[\!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]/);
        const type = match ? match[1] : 'NOTE';
        
        // Collect following quote lines
        let alertContent = [];
        let j = i + 1;
        while (j < lines.length && lines[j].trim().startsWith('>')) {
          const contentLine = lines[j].replace(/^>\s?/, '');
          alertContent.push(contentLine);
          j++;
        }
        i = j - 1; // Advance main loop

        // Style mapping
        const styles = {
          NOTE: 'bg-blue-950/20 border-blue-500/30 text-blue-300',
          TIP: 'bg-emerald-950/20 border-emerald-500/30 text-emerald-300',
          IMPORTANT: 'bg-purple-950/20 border-purple-500/30 text-purple-300',
          WARNING: 'bg-amber-950/20 border-amber-500/30 text-amber-300',
          CAUTION: 'bg-rose-950/20 border-rose-500/30 text-rose-300'
        };

        const titles = {
          NOTE: '提示',
          TIP: '建议',
          IMPORTANT: '重要',
          WARNING: '警告',
          CAUTION: '注意'
        };

        elements.push(
          <div key={`alert-${i}`} className={`my-4 flex flex-col gap-1 rounded-xl border-l-4 px-4 py-3 text-xs leading-relaxed ${styles[type] || styles.NOTE}`}>
            <span className="font-semibold">{titles[type] || 'Note'}</span>
            <div>
              {alertContent.map((l, idx) => (
                <p key={idx}>{parseInlineElements(l)}</p>
              ))}
            </div>
          </div>
        );
      }
      
      // Handle Unordered Lists
      else if (line.trim().startsWith('- ') || line.trim().startsWith('* ')) {
        const cleanedLine = line.trim().slice(2);
        if (!inList || listType !== 'unordered') {
          commitList(i);
          inList = true;
          listType = 'unordered';
        }
        listItems.push(
          <li key={`li-${i}`} className="text-slate-300 text-sm">
            {parseInlineElements(cleanedLine)}
          </li>
        );
      } 
      
      // Handle Ordered Lists
      else if (/^\d+\.\s/.test(line.trim())) {
        const cleanedLine = line.trim().replace(/^\d+\.\s/, '');
        if (!inList || listType !== 'ordered') {
          commitList(i);
          inList = true;
          listType = 'ordered';
        }
        listItems.push(
          <li key={`li-${i}`} className="text-slate-300 text-sm">
            {parseInlineElements(cleanedLine)}
          </li>
        );
      } 
      
      // Handle Normal Text Line
      else {
        if (line.trim() === '') {
          commitList(i);
        } else {
          commitList(i);
          elements.push(
            <p key={i} className="my-2.5 text-slate-300 leading-relaxed text-sm">
              {parseInlineElements(line)}
            </p>
          );
        }
      }
    }

    commitList(lines.length); // clear trailing list items if any
    return elements;
  };

  const renderMarkdown = (text) => {
    // Regex splits text into code blocks and normal paragraphs
    const parts = text.split(/(```[\s\S]*?```)/g);
    return parts.map((part, index) => {
      if (part.startsWith('```')) {
        const match = part.match(/```(\w*)\n([\s\S]*?)```/);
        const lang = match ? match[1] : 'text';
        const code = match ? match[2] : part.slice(3, -3);
        return <CodeBlock key={index} code={code.trim()} language={lang || 'plaintext'} />;
      } else {
        return (
          <div key={index} className="my-1.5">
            {parseParagraphs(part)}
          </div>
        );
      }
    });
  };

  return (
    <div className="flex flex-1 flex-col overflow-y-auto px-4 py-6 md:px-8 lg:px-12 scrollbar-thin">
      {/* If empty chat, show welcome greeting */}
      {messages.length === 0 ? (
        <div className="mx-auto flex max-w-3xl flex-col items-center justify-center flex-1 text-center py-10 md:py-16">
          {/* Main Greeting */}
          <h1 className="text-4xl font-semibold tracking-tight md:text-5xl text-center w-full mb-4">
            <span className="bg-gradient-to-r from-blue-400 via-purple-400 to-rose-400 bg-clip-text text-transparent animate-gradient">
              Mozi,
            </span>
            <br />
            <span className="text-[#444746] dark:text-[#e3e3e3]">想了解什么，尽管问吧！</span>
          </h1>
        </div>
      ) : (
        /* Else, render conversation message logs */
        <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 py-4">
          <div className="text-xs font-semibold text-slate-500 uppercase tracking-widest border-b border-[#2d2f31]/60 pb-3 mb-2 flex items-center gap-2">
            <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={2} stroke="currentColor" className="h-4 w-4 text-cyan-400">
              <path strokeLinecap="round" strokeLinejoin="round" d="M7.5 8.25h9m-9 3H12m-9.75 1.51c0 1.6 1.123 2.994 2.707 3.227 1.129.166 2.27.293 3.423.379.35.026.67.21.865.501L12 21l2.755-4.133a1.14 1.14 0 01.865-.501 48.172 48.172 0 003.423-.379c1.584-.233 2.707-1.626 2.707-3.228V6.741c0-1.602-1.123-2.995-2.707-3.228A48.394 48.394 0 0012 3c-2.392 0-4.744.175-7.043.513C3.373 3.746 2.25 5.14 2.25 6.741v6.018z" />
            </svg>
            <span>{activeChatTitle}</span>
          </div>

          {messages.map((m) => (
            <div
              key={m.id}
              className={`flex gap-4 ${m.role === 'user' ? 'justify-end' : 'justify-start'}`}
            >
              {/* AI Avatar */}
              {m.role === 'assistant' && (
                <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-gradient-to-tr from-cyan-400 via-purple-500 to-pink-500 text-white shadow-md shadow-purple-500/20">
                  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" className="h-4.5 w-4.5 animate-pulse">
                    <path d="M11.645 20.91l-.007-.003-.022-.012a15.247 15.247 0 01-.383-.218 25.18 25.18 0 01-4.244-3.17C4.688 15.36 2.25 12.174 2.25 8.25 2.25 5.322 4.714 3 7.688 3A5.5 5.5 0 0112 5.052 5.5 5.5 0 0116.313 3c2.973 0 5.437 2.322 5.437 5.25 0 3.925-2.438 7.111-4.739 9.256a25.175 25.175 0 01-4.244 3.17 15.247 15.247 0 01-.383.219l-.022.012-.007.004-.003.001a.752.752 0 01-.704 0l-.003-.001z" />
                  </svg>
                </div>
              )}

              {/* Message Content */}
              {m.role === 'user' ? (
                <div className="rounded-3xl bg-[#1e1f20] px-5 py-3.5 text-sm leading-relaxed text-slate-200 max-w-[80%] break-words border border-[#2d2f31]/30 hover:border-slate-500/30 transition">
                  {m.text}
                </div>
              ) : (
                <div className="flex-1 overflow-hidden space-y-1.5">
                  {/* Streaming indicator or response body */}
                  <div className="prose prose-invert max-w-none text-slate-300">
                    {renderMarkdown(m.text)}
                  </div>
                </div>
              )}
            </div>
          ))}

          {/* Sparkle glowing text animation during active response generation */}
          {isGenerating && messages[messages.length - 1]?.role === 'user' && (
            <div className="flex gap-4 items-start justify-start">
              <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-gradient-to-tr from-cyan-400 via-purple-500 to-pink-500 text-white shadow-md shadow-purple-500/20 animate-spin">
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" className="h-4.5 w-4.5">
                  <path d="M11.645 20.91l-.007-.003-.022-.012a15.247 15.247 0 01-.383-.218 25.18 25.18 0 01-4.244-3.17C4.688 15.36 2.25 12.174 2.25 8.25 2.25 5.322 4.714 3 7.688 3A5.5 5.5 0 0112 5.052 5.5 5.5 0 0116.313 3c2.973 0 5.437 2.322 5.437 5.25 0 3.925-2.438 7.111-4.739 9.256a25.175 25.175 0 01-4.244 3.17 15.247 15.247 0 01-.383.219l-.022.012-.007.004-.003.001a.752.752 0 01-.704 0l-.003-.001z" />
                </svg>
              </div>
              <div className="flex flex-col gap-2 w-full pt-1.5">
                <div className="h-4 bg-[#2d2f31] rounded animate-pulse w-3/4"></div>
                <div className="h-4 bg-[#2d2f31] rounded animate-pulse w-1/2"></div>
                <div className="h-4 bg-[#2d2f31] rounded animate-pulse w-5/6"></div>
              </div>
            </div>
          )}
        </div>
      )}

      {/* Auto-scroll Target */}
      <div ref={messagesEndRef} />
    </div>
  );
}
