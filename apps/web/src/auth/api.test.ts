import { describe, expect, it } from 'vitest';
import { decodeAccounts, decodeStatus, validPassword, validUsername } from './api';

const account = {
  user_id: '019a0000-0000-7000-8000-000000000001',
  username: 'master',
  is_master: true,
  enabled: true,
  must_change_password: false,
};

describe('local account boundary', () => {
  it('requires a supported complete authenticated identity', () => {
    expect(decodeStatus({ schema_version: 1, mode: 'setup' }).mode).toBe('setup');
    expect(decodeStatus({ schema_version: 1, mode: 'authenticated', account }).account).toEqual(
      account,
    );
    for (const input of [
      { schema_version: 2, mode: 'setup' },
      { schema_version: 1, mode: 'authenticated' },
      { schema_version: 1, mode: 'authenticated', account: { ...account, is_master: 'true' } },
      { schema_version: 1, mode: 'authenticated', account: { ...account, user_id: '../admin' } },
      {
        schema_version: 1,
        mode: 'authenticated',
        account: { ...account, must_change_password: true },
      },
      { schema_version: 1, mode: 'password_change', account },
    ])
      expect(() => decodeStatus(input)).toThrow();
  });
  it('bounds account lists and refuses duplicate identities', () => {
    expect(decodeAccounts({ schema_version: 1, accounts: [account] })).toEqual([account]);
    expect(() => decodeAccounts({ schema_version: 1, accounts: [account, account] })).toThrow();
    expect(() =>
      decodeAccounts({ schema_version: 1, accounts: Array(1001).fill(account) }),
    ).toThrow();
  });
  it('does not silently change passwords or login identifiers', () => {
    expect(validUsername('office.master')).toBe(true);
    for (const input of ['ab', ' admin', 'ADMIN', 'a'.repeat(33), 'админ'])
      expect(validUsername(input)).toBe(false);
    expect(validPassword('  office secret  ')).toBe(true);
    expect(validPassword('🔐'.repeat(12))).toBe(true);
    expect(validPassword('a'.repeat(11))).toBe(false);
    expect(validPassword('a'.repeat(129))).toBe(false);
  });
});
