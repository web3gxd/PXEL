import type { ShareTtl } from './api';
export const TTL_OPTIONS: readonly { value: ShareTtl; label: string; seconds: number | null }[] = [
  { value: 'hour', label: '1 час', seconds: 3600 },
  { value: 'day', label: '1 день', seconds: 86400 },
  { value: 'week', label: '1 неделя', seconds: 604800 },
  { value: 'indefinite', label: 'Бессрочно', seconds: null },
];
export const INDEFINITE_CONFIRMATION =
  'Я понимаю, что эта ссылка не имеет срока действия, и отзову её, когда доступ больше не нужен';
export function validPassword(value: string): boolean {
  const length = [...value].length;
  return length >= 10 && length <= 128;
}
export function generatePin(
  random: (bytes: Uint8Array<ArrayBuffer>) => Uint8Array = (bytes) => crypto.getRandomValues(bytes),
): string {
  let pin = '';
  while (pin.length < 6) {
    for (const byte of random(new Uint8Array(16))) {
      if (byte < 250) pin += String(byte % 10);
      if (pin.length === 6) break;
    }
  }
  return pin;
}
export function changePin(
  current: string,
  index: number,
  value: string,
): { pin: string; focus: number } {
  const digits = value.replace(/[^0-9]/g, '').slice(0, 6 - index);
  const cells = Array.from({ length: 6 }, (_, i) => current[i] ?? ' ');
  if (!digits) cells[index] = ' ';
  else
    for (let offset = 0; offset < digits.length; offset++)
      cells[index + offset] = digits[offset] ?? '';
  return { pin: cells.join(''), focus: Math.min(5, index + digits.length) };
}
export const formatExpiry = (seconds: number | null) =>
  seconds === null
    ? 'Без срока действия'
    : new Intl.DateTimeFormat('ru', { dateStyle: 'medium', timeStyle: 'short' }).format(
        new Date(seconds * 1000),
      );
