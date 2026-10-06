import { fromMarkdown } from 'mdast-util-from-markdown';
import { gfmFromMarkdown } from 'mdast-util-gfm';
import { gfm } from 'micromark-extension-gfm';
import { ClipboardError, type CellMatrix, normalizeBreaks } from './tabular';

export type MarkdownTable = {
  number: number;
  heading: string;
  context: string;
  rows: number;
  columns: number;
  cells?: CellMatrix;
  sample: CellMatrix;
  alignment: (string | null)[];
  tooLarge: boolean;
};
export type MarkdownDocument = { recognized: boolean; tables: MarkdownTable[] };
type Node = {
  type: string;
  value?: string;
  url?: string;
  alt?: string;
  children?: Node[];
  align?: (string | null)[];
  position?: { start?: { offset?: number }; end?: { offset?: number } };
};
const MAX_BYTES = 16 * 1024 * 1024,
  MAX_TABLES = 32,
  MAX_CELLS = 1000;
export function parseMarkdown(text: string): MarkdownDocument {
  if (new TextEncoder().encode(text).length > MAX_BYTES) throw new ClipboardError('limit');
  const tree = fromMarkdown(text, {
    extensions: [gfm()],
    mdastExtensions: [gfmFromMarkdown()],
  }) as unknown as Node;
  const blocks = tree.children ?? [];
  const tables: MarkdownTable[] = [];
  let heading = '';
  let meaningful = blocks.length > 1;
  const inline = (node: Node): string => {
    if (node.type === 'text' || node.type === 'inlineCode') return node.value ?? '';
    if (node.type === 'break') return '\n';
    if (node.type === 'image') return node.alt ?? '';
    if (node.type === 'html') return /^<br\s*\/?>$/i.test((node.value ?? '').trim()) ? '\n' : '';
    if (
      node.type === 'link' ||
      ['emphasis', 'strong', 'delete', 'linkReference', 'footnoteReference', 'footnote'].includes(
        node.type,
      )
    )
      return (node.children ?? []).map(inline).join('');
    if (node.type === 'paragraph' || node.type === 'heading' || node.type === 'tableCell')
      return (node.children ?? []).map(inline).join('');
    return '';
  };
  const visit = (nodes: Node[]) => {
    for (const node of nodes) {
      if (node.type === 'heading') heading = inline(node);
      if (node.type === 'table') {
        if (tables.length >= MAX_TABLES) throw new ClipboardError('limit');
        const rows = node.children ?? [];
        const width = Math.max(0, ...rows.map((row) => row.children?.length ?? 0));
        const height = rows.length;
        const tooLarge = !width || width * height > MAX_CELLS;
        const readRows = (limit: number) =>
          rows
            .slice(0, limit)
            .map((row) =>
              Array.from({ length: Math.min(width, 6) }, (_, column) =>
                inline(row.children?.[column] ?? { type: 'text' }),
              ),
            );
        const cells = tooLarge
          ? undefined
          : rows.map((row) =>
              Array.from({ length: width }, (_, column) =>
                inline(row.children?.[column] ?? { type: 'text' }),
              ),
            );
        const start = Math.max(0, (node.position?.start?.offset ?? 0) - 100),
          end = Math.min(text.length, node.position?.end?.offset ?? text.length);
        tables.push({
          number: tables.length + 1,
          heading,
          context: text.slice(start, Math.min(end + 100, start + 500)).trim(),
          rows: height,
          columns: width,
          ...(cells ? { cells } : {}),
          sample: readRows(4),
          alignment: node.align ?? [],
          tooLarge,
        });
      } else if (node.children) {
        if (!['root', 'paragraph', 'text', 'break'].includes(node.type)) meaningful = true;
        if (
          node.type === 'inlineCode' ||
          node.type === 'link' ||
          node.type === 'linkReference' ||
          node.type === 'emphasis' ||
          node.type === 'strong' ||
          node.type === 'delete' ||
          node.type === 'image'
        )
          meaningful = true;
        visit(node.children);
      } else if (
        ['code', 'inlineCode', 'html', 'thematicBreak', 'list', 'blockquote'].includes(node.type)
      )
        meaningful = true;
    }
  };
  visit(blocks);
  return { recognized: tables.length > 0 || meaningful, tables };
}

export function toClipboardMarkdownValue(value: string | undefined) {
  return normalizeBreaks(value ?? '');
}
