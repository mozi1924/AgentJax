import React, { cloneElement, isValidElement } from 'react';
import type { ReactNode } from 'react';
import CodeBlock from '../CodeBlock';
import { OverlayScrollArea } from '../OverlayScrollArea';

const parseInlineElements = (text: string): ReactNode[] => {
  const parts = text.split(/(\*\*.*?\*\*|`.*?`)/g);
  return parts.map((part, index) => {
    if (part.startsWith('**') && part.endsWith('**')) {
      return (
        <strong key={index} className="font-semibold text-slate-100">
          {part.slice(2, -2)}
        </strong>
      );
    }
    if (part.startsWith('`') && part.endsWith('`')) {
      return (
        <code
          key={index}
          className="mx-0.5 rounded bg-[#2d2f31] px-1.5 py-0.5 font-mono text-xs text-cyan-300"
        >
          {part.slice(1, -1)}
        </code>
      );
    }
    return part;
  });
};

const parseParagraphs = (rawText: string, t?: (key: string, replacements?: Record<string, string>) => string) => {
  const lines = rawText.split('\n');
  const elements: ReactNode[] = [];
  let listItems: ReactNode[] = [];
  let inList = false;
  let listType: 'unordered' | 'ordered' = 'unordered';

  const commitList = (key: number) => {
    if (listItems.length === 0) return;

    if (listType === 'ordered') {
      elements.push(
        <ol key={`ol-${key}`} className="my-2 list-decimal space-y-1 pl-6">
          {listItems}
        </ol>
      );
    } else {
      elements.push(
        <ul key={`ul-${key}`} className="my-2 list-disc space-y-1 pl-6">
          {listItems}
        </ul>
      );
    }
    listItems = [];
    inList = false;
  };

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];

    if (line.trim().startsWith('|') && lines[i + 1]?.trim().startsWith('|')) {
      commitList(i);

      const tableLines: string[] = [];
      let j = i;
      while (j < lines.length && lines[j].trim().startsWith('|')) {
        tableLines.push(lines[j]);
        j += 1;
      }
      i = j - 1;

      const headerLine = tableLines[0];
      const rowLines = tableLines.slice(2);
      const parseTableRow = (rowText: string) =>
        rowText
          .split('|')
          .slice(1, -1)
          .map((cell) => cell.trim());

      const headers = parseTableRow(headerLine);
      const rows = rowLines.map(parseTableRow);

      elements.push(
        <OverlayScrollArea
          key={`table-${i}`}
          axis="horizontal"
          containerClassName="my-4"
          className="rounded-xl border border-[#2d2f31]"
        >
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
        </OverlayScrollArea>
      );
      continue;
    }

    if (line.startsWith('### ')) {
      commitList(i);
      elements.push(
        <h3 key={i} className="mt-5 mb-2 text-base font-semibold text-slate-100">
          {parseInlineElements(line.slice(4))}
        </h3>
      );
      continue;
    }

    if (line.startsWith('#### ')) {
      commitList(i);
      elements.push(
        <h4 key={i} className="mt-4 mb-1 text-sm font-semibold text-slate-200">
          {parseInlineElements(line.slice(5))}
        </h4>
      );
      continue;
    }

    if (line.startsWith('## ')) {
      commitList(i);
      elements.push(
        <h2
          key={i}
          className="mt-6 mb-3 border-b border-[#2d2f31] pb-1.5 text-lg font-semibold text-slate-100"
        >
          {parseInlineElements(line.slice(3))}
        </h2>
      );
      continue;
    }

    if (line.startsWith('---') || line.startsWith('***')) {
      commitList(i);
      elements.push(<hr key={i} className="my-5 border-t border-[#2d2f31]" />);
      continue;
    }

    if (line.startsWith('> [!')) {
      commitList(i);
      const match = line.match(/^> \[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]/);
      const type = match ? match[1] : 'NOTE';

      const alertContent: string[] = [];
      let j = i + 1;
      while (j < lines.length && lines[j].trim().startsWith('>')) {
        const contentLine = lines[j].replace(/^>\s?/, '');
        alertContent.push(contentLine);
        j += 1;
      }
      i = j - 1;

      const styles: Record<string, string> = {
        NOTE: 'bg-blue-950/20 border-blue-500/30 text-blue-300',
        TIP: 'bg-emerald-950/20 border-emerald-500/30 text-emerald-300',
        IMPORTANT: 'bg-purple-950/20 border-purple-500/30 text-purple-300',
        WARNING: 'bg-amber-950/20 border-amber-500/30 text-amber-300',
        CAUTION: 'bg-rose-950/20 border-rose-500/30 text-rose-300',
      };

      const titles: Record<string, string> = {
        NOTE: t ? t('markdown.alert.note') : '提示',
        TIP: t ? t('markdown.alert.tip') : '建议',
        IMPORTANT: t ? t('markdown.alert.important') : '重要',
        WARNING: t ? t('markdown.alert.warning') : '警告',
        CAUTION: t ? t('markdown.alert.caution') : '注意',
      };

      elements.push(
        <div
          key={`alert-${i}`}
          className={`my-4 flex flex-col gap-1 rounded-xl border-l-4 px-4 py-3 text-xs leading-relaxed ${
            styles[type] || styles.NOTE
          }`}
        >
          <span className="font-semibold">{titles[type] || 'Note'}</span>
          <div>
            {alertContent.map((alertLine, idx) => (
              <p key={idx}>{parseInlineElements(alertLine)}</p>
            ))}
          </div>
        </div>
      );
      continue;
    }

    if (line.trim().startsWith('- ') || line.trim().startsWith('* ')) {
      const cleanedLine = line.trim().slice(2);
      if (!inList || listType !== 'unordered') {
        commitList(i);
        inList = true;
        listType = 'unordered';
      }
      listItems.push(
        <li key={`li-${i}`} className="text-sm text-slate-300">
          {parseInlineElements(cleanedLine)}
        </li>
      );
      continue;
    }

    if (/^\d+\.\s/.test(line.trim())) {
      const cleanedLine = line.trim().replace(/^\d+\.\s/, '');
      if (!inList || listType !== 'ordered') {
        commitList(i);
        inList = true;
        listType = 'ordered';
      }
      listItems.push(
        <li key={`li-${i}`} className="text-sm text-slate-300">
          {parseInlineElements(cleanedLine)}
        </li>
      );
      continue;
    }

    if (line.trim() === '') {
      commitList(i);
      continue;
    }

    commitList(i);
    elements.push(
      <p key={i} className="my-2.5 text-sm leading-relaxed text-slate-300">
        {parseInlineElements(line)}
      </p>
    );
  }

  commitList(lines.length);
  return elements;
};

const injectSuffixIntoLastElement = (elements: ReactNode[], suffix: ReactNode): ReactNode[] => {
  if (elements.length === 0 || !suffix) return elements;

  const lastIdx = elements.length - 1;
  const lastElement = elements[lastIdx];

  if (isValidElement(lastElement)) {
    const el = lastElement as React.ReactElement<any>;
    const { children, ...props } = el.props;

    if (el.type === 'p') {
      return [
        ...elements.slice(0, lastIdx),
        cloneElement(el, props, [
          ...(Array.isArray(children) ? children : [children]),
          suffix,
        ]),
      ];
    } else if (el.type === 'div' && el.props.className === 'my-1.5') {
      const nestedChildren = Array.isArray(children) ? children : [children];
      const updatedNested = injectSuffixIntoLastElement(nestedChildren, suffix);
      return [
        ...elements.slice(0, lastIdx),
        cloneElement(el, props, updatedNested),
      ];
    }
  }

  return [...elements, suffix];
};

export const renderMarkdown = (
  text: string,
  inlineSuffix?: ReactNode,
  t?: (key: string, replacements?: Record<string, string>) => string
) => {
  const parts = text.split(/(```[\s\S]*?```)/g);
  const elements = parts.map((part, index) => {
    if (part.startsWith('```')) {
      const match = part.match(/```(\w*)\n([\s\S]*?)```/);
      const lang = match ? match[1] : 'text';
      const code = match ? match[2] : part.slice(3, -3);
      return <CodeBlock key={index} code={code.trim()} language={lang || 'plaintext'} />;
    }

    return (
      <div key={index} className="my-1.5">
        {parseParagraphs(part, t)}
      </div>
    );
  });

  if (inlineSuffix) {
    return injectSuffixIntoLastElement(elements, inlineSuffix);
  }
  return elements;
};
