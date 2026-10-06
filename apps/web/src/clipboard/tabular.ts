export type CellMatrix = string[][];
export class ClipboardError extends Error {
  constructor(public readonly code: 'quoting' | 'limit' | 'table') {
    super(
      code === 'quoting'
        ? 'Некорректные кавычки в таблице.'
        : code === 'limit'
          ? 'Данные превышают лимиты вставки.'
          : 'Некорректная HTML-таблица.',
    );
  }
}
export const normalizeBreaks = (text: string) => text.replace(/\r\n?/g, '\n');

/** Delimiters terminate fields only outside quoted values. A final record separator
 * terminates the last record, rather than introducing a phantom blank record. */
export function parseDelimited(
  text: string,
  delimiter: '\t' | ',' = '\t',
  preserveFinalBlank = false,
): CellMatrix {
  if (new TextEncoder().encode(text).length > 1048576) throw new ClipboardError('limit');
  const rows: CellMatrix = [];
  let row: string[] = [],
    field = '',
    state: 'start' | 'plain' | 'quoted' | 'closed' = 'start';
  let terminated = false,
    cells = 0;
  const push = () => {
    if (++cells > 1000) throw new ClipboardError('limit');
    row.push(normalizeBreaks(field));
    field = '';
    state = 'start';
  };
  for (let i = 0; i < text.length; i++) {
    const ch = text[i]!;
    terminated = false;
    if (state === 'quoted') {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i++;
        } else state = 'closed';
      } else field += ch;
      continue;
    }
    if (ch === delimiter) {
      push();
      continue;
    }
    if (ch === '\r' || ch === '\n') {
      if (ch === '\r' && text[i + 1] === '\n') i++;
      push();
      rows.push(row);
      row = [];
      terminated = true;
      continue;
    }
    if (state === 'closed') throw new ClipboardError('quoting');
    if (state === 'start' && ch === '"') state = 'quoted';
    else {
      field += ch;
      state = 'plain';
    }
  }
  if (state === 'quoted') throw new ClipboardError('quoting');
  if (!terminated || preserveFinalBlank) {
    push();
    rows.push(row);
  }
  return rectangular(rows);
}
export function rectangular(rows: CellMatrix): CellMatrix {
  const width = Math.max(0, ...rows.map((row) => row.length));
  if (!width || rows.length * width > 1000) throw new ClipboardError('limit');
  return rows.map((row) => [...row, ...Array<string>(width - row.length).fill('')]);
}
export function serializeDelimited(rows: CellMatrix): string {
  return rows
    .map((row) =>
      row
        .map((value) =>
          /["\t\r\n]/.test(value) ? `"${normalizeBreaks(value).replaceAll('"', '""')}"` : value,
        )
        .join('\t'),
    )
    .join('\n');
}
const escapeHtml = (value: string) =>
  value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
export function serializeHtml(rows: CellMatrix): string {
  return `<table>${rows.map((row) => `<tr>${row.map((value) => `<td style="white-space:pre-wrap">${escapeHtml(normalizeBreaks(value)).replaceAll('\n', '<br>')}</td>`).join('')}</tr>`).join('')}</table>`;
}

/** Detached template content is inert: no resource loads, scripts, or DOM insertion. */
export function parseHtmlTable(html: string): CellMatrix | undefined {
  if (new TextEncoder().encode(html).length > 1048576) throw new ClipboardError('limit');
  const template = document.createElement('template');
  template.innerHTML = html;
  const table = template.content.querySelector('table');
  if (!table) return undefined;
  const rows: CellMatrix = [];
  const occupied = new Set<string>();
  const text = (node: Node): string => {
    if (node.nodeType === Node.TEXT_NODE) return node.textContent ?? '';
    if (!(node instanceof Element)) return '';
    if (['SCRIPT', 'STYLE', 'IFRAME', 'OBJECT', 'TEMPLATE'].includes(node.tagName)) return '';
    if (node.tagName === 'BR') return '\n';
    let result = Array.from(node.childNodes, text).join('');
    if (['P', 'DIV', 'LI'].includes(node.tagName) && node.nextSibling) result += '\n';
    return result;
  };
  const trs = [...table.querySelectorAll('tr')].filter((tr) => tr.closest('table') === table);
  if (trs.length > 1000) throw new ClipboardError('limit');
  for (let r = 0; r < trs.length; r++) {
    const row = rows[r] ?? (rows[r] = []);
    let c = 0;
    for (const cell of [...trs[r]!.children].filter((cell) =>
      ['TD', 'TH'].includes(cell.tagName),
    )) {
      while (occupied.has(`${r}/${c}`)) c++;
      const span = (key: string) => {
        const value = Number(cell.getAttribute(key) ?? '1');
        if (!Number.isInteger(value) || value < 1 || value > 1000)
          throw new ClipboardError('table');
        return value;
      };
      const height = span('rowspan'),
        width = span('colspan');
      if ((r + height) * (c + width) > 1000) throw new ClipboardError('limit');
      for (let y = r; y < r + height; y++)
        for (let x = c; x < c + width; x++) {
          if (occupied.has(`${y}/${x}`)) throw new ClipboardError('table');
          occupied.add(`${y}/${x}`);
          (rows[y] ??= [])[x] =
            y === r && x === c ? normalizeBreaks(Array.from(cell.childNodes, text).join('')) : '';
        }
      c += width;
    }
    for (let x = 0; x < row.length; x++) row[x] ??= '';
  }
  return rectangular(rows);
}
