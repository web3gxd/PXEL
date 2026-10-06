import type { DocumentStore } from '../document/store';
import { parseDelimited, serializeHtml } from './tabular';

type InternalClipboard = {
  schema_version: 1;
  tsv?: string | undefined;
  formula?: string | undefined;
  range?: string | undefined;
  cut?: string | undefined;
};
const encode = (value: string) =>
  value
    .replaceAll('&', '&amp;')
    .replaceAll('"', '&quot;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;');
export function clipboardHtml(text: string, payload: InternalClipboard) {
  return serializeHtml(parseDelimited(text, '\t', true)).replace(
    '<table',
    `<table data-pxel-clipboard="${encode(JSON.stringify(payload))}"`,
  );
}
export function readInternalClipboard(html: string): InternalClipboard | undefined {
  if (html.length > 2 * 1024 * 1024) return undefined;
  const template = document.createElement('template');
  template.innerHTML = html;
  const encoded = template.content
    .querySelector('table[data-pxel-clipboard]')
    ?.getAttribute('data-pxel-clipboard');
  return decodeInternalClipboard(encoded);
}
export function decodeInternalClipboard(
  encoded: string | null | undefined,
): InternalClipboard | undefined {
  if (!encoded || encoded.length > 1024 * 1024) return undefined;
  try {
    const item = JSON.parse(encoded) as Record<string, unknown>;
    if (
      !item ||
      item.schema_version !== 1 ||
      !['tsv', 'formula', 'range', 'cut'].every(
        (key) => item[key] === undefined || typeof item[key] === 'string',
      )
    )
      return undefined;
    return item as InternalClipboard;
  } catch {
    return undefined;
  }
}
export async function writeSelectionClipboard(
  document: DocumentStore,
  action: 'copy' | 'cut',
  anonymize = false,
) {
  const selection = JSON.stringify([
    document.selected,
    document.selectionAnchor,
    document.activeSheetId,
  ]);
  try {
    // Start audit and ClipboardItem promises within the native user activation.
    const prepared = document.authorizeClipboard(action, anonymize).then((receipt) => {
      if (
        JSON.stringify([document.selected, document.selectionAnchor, document.activeSheetId]) !==
          selection ||
        document.editing
      )
        throw new Error('Выделение изменилось. Повторите копирование.');
      const text = receipt.projection
        ? document.copyRedactedSelectionTsv(receipt.projection)
        : document.copySelectionTsv();
      if (text === undefined) throw new Error('Выделение недоступно для копирования.');
      const cut = action === 'cut' ? document.cutRangePayload(false) : undefined;
      if (action === 'cut' && !cut) throw new Error('Выделение недоступно для вырезания.');
      const payload: InternalClipboard = {
        schema_version: 1,
        ...(cut
          ? { cut }
          : {
              tsv: JSON.stringify({ schema_version: 1, text }),
              ...(receipt.anonymized || document.projection?.representation === 'anonymized'
                ? {}
                : { formula: document.copyFormulaPayload(), range: document.copyRangePayload() }),
            }),
      };
      return { text, html: clipboardHtml(text, payload), cut };
    });
    // Consume both rejection paths even if the browser refuses clipboard access.
    const plain = prepared.then((value) => new Blob([value.text], { type: 'text/plain' }));
    const html = prepared.then((value) => new Blob([value.html], { type: 'text/html' }));
    void plain.catch(() => {});
    void html.catch(() => {});
    await navigator.clipboard.write([
      new ClipboardItem({ 'text/plain': plain, 'text/html': html }),
    ]);
    const value = await prepared;
    if (action === 'copy') {
      document.observeAssistant({ kind: 'copy' });
      document.notify();
    }
    if (value.cut) document.activateCutClipboard(value.cut);
    else document.reconcileCutClipboard('');
  } catch (error) {
    document.error = error instanceof Error ? error.message : 'Не удалось скопировать выделение.';
    document.notify();
  }
}
export async function writeTextClipboard(
  document: DocumentStore,
  text: string,
  action: 'copy' | 'cut',
  afterCut?: () => void,
  protectedView: boolean | 'unmapped' = false,
) {
  try {
    const authorized = document.authorizeClipboard(action, false, text).then((receipt) => {
      if (protectedView && receipt.anonymized)
        throw new Error(
          protectedView === 'unmapped'
            ? 'Для этого представления требуется отдельная обезличенная копия. Используйте безопасное копирование выделения таблицы.'
            : 'Для истории требуется отдельная обезличенная копия. Откройте текущую книгу и используйте безопасное копирование.',
        );
      return new Blob([receipt.anonymized ? (receipt.text ?? '') : text], { type: 'text/plain' });
    });
    void authorized.catch(() => {});
    await navigator.clipboard.write([new ClipboardItem({ 'text/plain': authorized })]);
    if (action === 'cut') afterCut?.();
  } catch (error) {
    document.error = error instanceof Error ? error.message : 'Не удалось скопировать текст.';
    document.notify();
  }
}
