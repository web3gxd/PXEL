import { useEffect, useRef, useState, type ReactNode } from 'react';
import { decodeSyntax, type FormulaToken } from './diagnostics';
export function FormulaSyntax({
  text,
  row,
  column,
  children,
}: {
  text: string;
  row: number;
  column: number;
  children: ReactNode;
}) {
  const [syntax, setSyntax] = useState<{ source: string; tokens: FormulaToken[] }>();
  const overlay = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (!text.startsWith('=')) return;
    let worker: Worker | undefined;
    const timer = setTimeout(() => {
      try {
        worker = new Worker(new URL('../calculation/diagnostics.worker.ts', import.meta.url), {
          type: 'module',
        });
        worker.onmessage = ({ data }: MessageEvent<{ syntax?: unknown }>) => {
          try {
            setSyntax({ source: text, tokens: decodeSyntax(text, data.syntax) });
          } catch {
            setSyntax(undefined);
          }
          worker?.terminate();
        };
        worker.onerror = () => {
          setSyntax(undefined);
          worker?.terminate();
        };
        worker.postMessage({ source: text, row, column });
      } catch {
        setSyntax(undefined);
      }
    }, 120);
    return () => {
      clearTimeout(timer);
      worker?.terminate();
    };
  }, [text, row, column]);
  const tokens = syntax?.source === text ? syntax.tokens : [];
  const pieces: ReactNode[] = [];
  let position = 0;
  for (const token of tokens) {
    pieces.push(text.slice(position, token.start));
    pieces.push(
      <span key={token.start} className={`syntax-${token.kind}`} data-token-kind={token.kind}>
        {text.slice(token.start, token.end)}
      </span>,
    );
    position = token.end;
  }
  pieces.push(text.slice(position));
  return (
    <div
      className={`formula-syntax${tokens.length ? ' formula-syntax-active' : ''}`}
      onScrollCapture={(event) => {
        if (overlay.current && event.target instanceof HTMLTextAreaElement)
          overlay.current.style.transform = `translate(${-event.target.scrollLeft}px, ${-event.target.scrollTop}px)`;
      }}
    >
      {tokens.length > 0 && (
        <pre ref={overlay} className="formula-syntax-overlay" aria-hidden="true">
          {pieces}
        </pre>
      )}
      {children}
    </div>
  );
}
