import { describe, expect, it } from 'vitest';
import { changePin, generatePin, TTL_OPTIONS, validPassword } from './policy';
describe('sharing input policy', () => {
  it('uses rejection sampling and retains leading zeroes', () => {
    const pin = generatePin((bytes) => {
      bytes.set([255, 254, 253, 252, 251, 250, 0, 1, 2, 3, 4, 5]);
      return bytes;
    });
    expect(pin).toBe('012345');
  });
  it('distributes pasted digits, advances focus and preserves an empty middle cell', () => {
    expect(changePin('', 0, '012345')).toEqual({ pin: '012345', focus: 5 });
    expect(changePin('123456', 2, '')).toEqual({ pin: '12 456', focus: 2 });
    expect(changePin('12 456', 2, '9')).toEqual({ pin: '129456', focus: 3 });
    expect(changePin('123456', 4, '78-90')).toEqual({ pin: '123478', focus: 5 });
    expect(changePin('', 0, '１２３')).toEqual({ pin: '      ', focus: 0 });
  });
  it('counts Unicode code points without trimming the password', () => {
    expect(validPassword('🔐'.repeat(10))).toBe(true);
    expect(validPassword('🔐'.repeat(9))).toBe(false);
    expect(validPassword(' '.repeat(10))).toBe(true);
    expect(validPassword('я'.repeat(128))).toBe(true);
    expect(validPassword('я'.repeat(129))).toBe(false);
  });
  it('offers exactly four TTLs with one day as the default position', () => {
    expect(TTL_OPTIONS.map((option) => option.value)).toEqual([
      'hour',
      'day',
      'week',
      'indefinite',
    ]);
    expect(TTL_OPTIONS[1]?.seconds).toBe(86400);
    expect(TTL_OPTIONS[3]?.seconds).toBeNull();
  });
});
