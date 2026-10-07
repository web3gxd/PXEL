import { boundedJson } from '../sharing/api';

export type XlsxPayload = { schema_version: 1; file_base64: string; selected_sheets?: number[] };
export type XlsxCell = {
  row: number;
  column: number;
  value:
    | { Text: string }
    | { Number: number }
    | { Boolean: boolean }
    | { Formula: string }
    | { FormulaCopy: { master_row: number; master_column: number } };
};
export const xlsxIssues = {
  UnsupportedPackagePart: 'Дополнительные части файла не поддерживаются.',
  WorkbookMetadata: 'Часть настроек книги не поддерживается.',
  WorksheetMetadata: 'Часть настроек листа не поддерживается.',
  RowFormatting: 'Оформление строк поддерживается не полностью.',
  CellFormatting: 'Оформление ячеек поддерживается не полностью.',
} as const;
export type XlsxPreview = {
  schema_version: 1;
  selected_sheets: number[];
  preview_digest: string;
  sheets: {
    index: number;
    name: string;
    cells: number;
    styles: number;
    merges: number;
    sample: XlsxCell[];
  }[];
  issues: { code: keyof typeof xlsxIssues; sheet_index: number | null }[];
};
const integer = (value: unknown, min: number, max: number): value is number =>
  typeof value === 'number' && Number.isSafeInteger(value) && value >= min && value <= max;
function validCell(cell: XlsxCell): boolean {
  if (
    !cell ||
    !integer(cell.row, 1, 1048576) ||
    !integer(cell.column, 1, 16384) ||
    !cell.value ||
    typeof cell.value !== 'object'
  )
    return false;
  const entries = Object.entries(cell.value);
  if (entries.length !== 1) return false;
  const [kind, value] = entries[0]!;
  if (kind === 'Text' || kind === 'Formula')
    return typeof value === 'string' && [...value].length <= 256;
  if (kind === 'Number') return typeof value === 'number' && Number.isFinite(value);
  if (kind === 'Boolean') return typeof value === 'boolean';
  if (kind !== 'FormulaCopy' || !value || typeof value !== 'object') return false;
  const copy = value as { master_row?: unknown; master_column?: unknown };
  return integer(copy.master_row, 1, 1048576) && integer(copy.master_column, 1, 16384);
}
export function decodeXlsxPreview(value: unknown): XlsxPreview {
  const v = value as XlsxPreview;
  if (
    !v ||
    v.schema_version !== 1 ||
    typeof v.preview_digest !== 'string' ||
    !/^[0-9a-f]{64}$/.test(v.preview_digest) ||
    !Array.isArray(v.sheets) ||
    !integer(v.sheets.length, 1, 64) ||
    !v.sheets.every(
      (sheet, index) =>
        sheet &&
        sheet.index === index &&
        typeof sheet.name === 'string' &&
        [...sheet.name].length > 0 &&
        [...sheet.name].length <= 31 &&
        integer(sheet.cells, 0, 100000) &&
        integer(sheet.styles, 0, 100000) &&
        integer(sheet.merges, 0, 100000) &&
        Array.isArray(sheet.sample) &&
        sheet.sample.length === Math.min(sheet.cells, 40) &&
        sheet.sample.every(validCell),
    ) ||
    !Array.isArray(v.selected_sheets) ||
    !v.selected_sheets.length ||
    !v.selected_sheets.every(
      (index, position) =>
        integer(index, 0, v.sheets.length - 1) &&
        (position === 0 || index > v.selected_sheets[position - 1]!),
    ) ||
    !Array.isArray(v.issues) ||
    v.issues.length > 128 ||
    !v.issues.every(
      (issue) =>
        issue &&
        Object.hasOwn(xlsxIssues, issue.code) &&
        (issue.sheet_index === null || integer(issue.sheet_index, 0, v.sheets.length - 1)),
    )
  )
    throw new Error('Некорректный предпросмотр XLSX.');
  return v;
}
export async function readXlsxFile(file: File): Promise<XlsxPayload> {
  if (!file.size || file.size > 32 * 1024 * 1024)
    throw new Error('Выберите XLSX-файл размером до 32 МиБ.');
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = '';
  for (let index = 0; index < bytes.length; index += 8192)
    binary += String.fromCharCode(...bytes.subarray(index, index + 8192));
  return { schema_version: 1, file_base64: btoa(binary) };
}
export async function previewXlsx(payload: XlsxPayload, signal: AbortSignal): Promise<XlsxPreview> {
  const response = await fetch('/api/imports/xlsx/preview', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
    signal: AbortSignal.any([signal, AbortSignal.timeout(60000)]),
  });
  if (!response.ok) {
    const error = (await boundedJson(response, 8192).catch(() => ({}))) as { code?: string };
    const messages: Record<string, string> = {
      xlsx_unavailable: 'Обработчик XLSX недоступен. Повторите попытку позже.',
      xlsx_rejected:
        'Файл повреждён, превышает ограничения или содержит неподдерживаемое активное содержимое.',
      xlsx_limit: 'Файл превышает ограничения XLSX.',
      xlsx_options: 'Выберите хотя бы один лист.',
      import_busy: 'Другой импорт или экспорт ещё выполняется. Повторите попытку.',
    };
    throw new Error(messages[error.code ?? ''] ?? 'Не удалось проверить XLSX. Повторите попытку.');
  }
  return decodeXlsxPreview(await boundedJson(response, 4 * 1024 * 1024));
}
export function xlsxCellText(value: XlsxCell['value']): string {
  if ('Text' in value) return value.Text;
  if ('Number' in value) return String(value.Number);
  if ('Boolean' in value) return value.Boolean ? 'TRUE' : 'FALSE';
  if ('Formula' in value) return '=' + value.Formula;
  return `Копия формулы: строка ${value.FormulaCopy.master_row}, столбец ${value.FormulaCopy.master_column}`;
}
