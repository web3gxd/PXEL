import { boundedJson, isUuid } from '../sharing/api';
import { decodeRedactedProjection, type Projection } from './store';
export const PRIVACY_TYPES = {
  phone: 'Телефоны РФ',
  passport: 'Паспорта',
  snils: 'СНИЛС',
  inn: 'ИНН',
  email: 'Email',
  card: 'Банковские карты',
} as const;
export type PrivacyKind = keyof typeof PRIVACY_TYPES;
export type PrivacyRules = { enabled: boolean; types: PrivacyKind[]; exceptions: string[] };
export type PrivacyPolicy = {
  schema_version: 1;
  revision: number;
  access_epoch: number;
  rules: PrivacyRules;
};
export type PrivacyFinding = {
  sheet_id: string;
  row_id: string;
  column_id: string;
  row: number;
  column: number;
  kind: PrivacyKind;
  fingerprint: string;
};
export type PrivacyReport = {
  schema_version: 1;
  document_version: string;
  policy_revision: number;
  affected: number;
  findings: PrivacyFinding[];
  projection: Projection;
};
export function privacyExceptionBatch(existing: string[], findings: PrivacyFinding[]) {
  const result = [...new Set([...existing, ...findings.map((f) => f.fingerprint)])];
  if (result.length > 1000)
    throw new Error('Не более 1000 исключений. Уменьшите диапазон; изменения не сохранены.');
  return result;
}
export function privacyRangeFindings(report: PrivacyReport, sheetId: string, range: string) {
  const match = /^([A-Z]{1,3})([1-9]\d*)(?::([A-Z]{1,3})([1-9]\d*))?$/.exec(
    range.trim().toUpperCase(),
  );
  if (!match) throw new Error('Введите диапазон, например B2:D20.');
  const column = (label: string) =>
    [...label].reduce((value, c) => value * 26 + c.charCodeAt(0) - 64, 0) - 1;
  const a = column(match[1]!),
    b = column(match[3] ?? match[1]!);
  const r = Number(match[2]) - 1,
    s = Number(match[4] ?? match[2]) - 1;
  const sheet = report.projection.sheets.find((item) => item.id === sheetId);
  if (!sheet || Math.max(r, s) >= sheet.rows.length || Math.max(a, b) >= sheet.columns.length)
    throw new Error('Диапазон выходит за границы листа.');
  return report.findings.filter(
    (f) =>
      f.sheet_id === sheetId &&
      f.row >= Math.min(r, s) &&
      f.row <= Math.max(r, s) &&
      f.column >= Math.min(a, b) &&
      f.column <= Math.max(a, b),
  );
}
const integer = (n: unknown): n is number =>
  typeof n === 'number' && Number.isSafeInteger(n) && n >= 0;
export function decodePrivacyPolicy(raw: unknown): PrivacyPolicy {
  const p = raw as PrivacyPolicy;
  if (
    !p ||
    p.schema_version !== 1 ||
    !integer(p.revision) ||
    !integer(p.access_epoch) ||
    !p.rules ||
    typeof p.rules.enabled !== 'boolean' ||
    !Array.isArray(p.rules.types) ||
    p.rules.types.length > 6 ||
    p.rules.types.some((t) => !(t in PRIVACY_TYPES)) ||
    new Set(p.rules.types).size !== p.rules.types.length ||
    !Array.isArray(p.rules.exceptions) ||
    p.rules.exceptions.length > 1000 ||
    p.rules.exceptions.some((f) => typeof f !== 'string' || !/^[0-9a-f]{64}$/.test(f)) ||
    new Set(p.rules.exceptions).size !== p.rules.exceptions.length
  )
    throw new Error('Неподдерживаемая политика распознавания.');
  return p;
}
export function decodePrivacyReport(raw: unknown): PrivacyReport {
  const p = raw as Record<string, unknown>;
  if (
    !p ||
    p.schema_version !== 1 ||
    typeof p.document_version !== 'string' ||
    !/^[1-9]\d*$/.test(p.document_version) ||
    !integer(p.policy_revision) ||
    !integer(p.affected) ||
    p.affected > 100000 ||
    !Array.isArray(p.findings) ||
    p.findings.length > 100000
  )
    throw new Error('Некорректный отчёт распознавания.');
  for (const raw of p.findings) {
    const f = raw as PrivacyFinding;
    if (
      !f ||
      ![f.sheet_id, f.row_id, f.column_id].every(isUuid) ||
      !integer(f.row) ||
      f.row >= 100000 ||
      !integer(f.column) ||
      f.column >= 1000 ||
      !(f.kind in PRIVACY_TYPES) ||
      typeof f.fingerprint !== 'string' ||
      !/^[0-9a-f]{64}$/.test(f.fingerprint)
    )
      throw new Error('Некорректная найденная ячейка.');
  }
  const projection = decodeRedactedProjection(p.projection);
  if (
    projection.version !== p.document_version ||
    projection.privacy_revision !== p.policy_revision
  )
    throw new Error('Версия безопасного представления не совпала.');
  return { ...p, projection } as PrivacyReport;
}
export async function readPrivacy(id: string, signal?: AbortSignal) {
  const r = await fetch(`/api/documents/${id}/privacy`, {
    cache: 'no-store',
    signal: signal ?? AbortSignal.timeout(10000),
  });
  if (!r.ok) throw new Error('Не удалось загрузить настройки распознавания.');
  return decodePrivacyPolicy(await boundedJson(r, 128 * 1024));
}
export async function savePrivacy(
  id: string,
  version: string,
  revision: number,
  rules: PrivacyRules,
) {
  const r = await fetch(`/api/documents/${id}/privacy`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      schema_version: 1,
      document_version: version,
      expected_revision: revision,
      rules,
    }),
    signal: AbortSignal.timeout(10000),
  });
  if (!r.ok)
    throw new Error(
      r.status === 409
        ? 'Версия или настройки изменились. Откройте окно заново.'
        : 'Не удалось сохранить настройки.',
    );
  return decodePrivacyPolicy(await boundedJson(r, 128 * 1024));
}
export async function analyzePrivacy(id: string, version: string) {
  const r = await fetch(`/api/documents/${id}/privacy/analyze`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ schema_version: 1, document_version: version }),
    signal: AbortSignal.timeout(30000),
  });
  if (!r.ok)
    throw new Error(
      r.status === 409
        ? 'Книга изменилась. Повторите анализ.'
        : 'Анализ недоступен: проверьте включённые типы и лимиты книги. Безопасная выдача заблокирована.',
    );
  return decodePrivacyReport(await boundedJson(r, 16 * 1024 * 1024));
}
