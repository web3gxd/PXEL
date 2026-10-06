import { afterEach, expect, it, vi } from 'vitest';
import { decodePolicy, decodeAuditPage, authorizeClipboard } from './disclosure';
afterEach(() => vi.unstubAllGlobals());
it('CSV audit entries require audit version 2', () => {
  const page = {
    schema_version: 2,
    next_cursor: null,
    events: [
      {
        event_id: '019a1234-5678-789a-8bcd-0123456789ab',
        actor_id: '019a1234-5678-789a-8bcd-0123456789ac',
        link_id: null,
        created_at: 1,
        document_version: 2,
        book_revision: 1,
        link_revision: 0,
        action: 'export',
        format: 'csv',
        allowed: true,
        reason: 'allowed',
      },
    ],
  };
  expect(decodeAuditPage(page).events[0]?.format).toBe('csv');
  expect(() => decodeAuditPage({ ...page, schema_version: 1 })).toThrow();
  expect(() => decodeAuditPage({ ...page, schema_version: 3 })).toThrow();
  const preparation = {
    ...page,
    events: [{ ...page.events[0], format: 'xlsx', allowed: false, reason: 'preparation_failed' }],
  };
  expect(decodeAuditPage(preparation).events[0]?.reason).toBe('preparation_failed');
  expect(() => decodeAuditPage({ ...preparation, schema_version: 1 })).toThrow();
});
it('rejects unknown versions, duplicate formats, and malformed stamps', () => {
  const policy = {
    schema_version: 1,
    revision: 0,
    access_epoch: 0,
    rules: {
      allow_copy: true,
      export_formats: ['xlsx'],
      allow_participant_export: false,
      require_anonymization: false,
    },
  };
  expect(decodePolicy(policy).rules.allow_copy).toBe(true);
  expect(() => decodePolicy({ ...policy, schema_version: 3 })).toThrow();
  expect(() =>
    decodePolicy({ ...policy, rules: { ...policy.rules, export_formats: ['xlsx', 'xlsx'] } }),
  ).toThrow();
});
it('CSV requires policy version 2 and rejects invented formats', () => {
  const policy = {
    schema_version: 2,
    revision: 1,
    access_epoch: 1,
    rules: {
      allow_copy: true,
      export_formats: ['csv'],
      allow_participant_export: false,
      require_anonymization: false,
    },
  };
  expect(decodePolicy(policy).rules.export_formats).toEqual(['csv']);
  expect(() => decodePolicy({ ...policy, schema_version: 1 })).toThrow();
  expect(() =>
    decodePolicy({ ...policy, rules: { ...policy.rules, export_formats: ['csv', 'csv'] } }),
  ).toThrow();
  expect(() =>
    decodePolicy({ ...policy, rules: { ...policy.rules, export_formats: ['html'] } }),
  ).toThrow();
});
it('requires a matching durable authorization and exposes no response bodies in errors', async () => {
  const stamp = { document_version: 2, book_revision: 1, link_revision: 0 };
  const fetch = vi
    .fn()
    .mockResolvedValueOnce(
      new Response(JSON.stringify({ schema_version: 1, allowed: true, reason: 'allowed', stamp })),
    )
    .mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          schema_version: 1,
          allowed: true,
          reason: 'allowed',
          stamp: { ...stamp, book_revision: 2 },
        }),
      ),
    )
    .mockResolvedValueOnce(new Response('secret marker', { status: 503 }));
  vi.stubGlobal('fetch', fetch);
  await authorizeClipboard('/api/documents/doc/clipboard/authorize', 'copy', stamp);
  expect(JSON.parse(String(fetch.mock.calls[0]?.[1]?.body))).toEqual({
    schema_version: 1,
    action: 'copy',
    stamp,
  });
  await expect(authorizeClipboard('/same', 'copy', stamp)).rejects.toThrow();
  await expect(authorizeClipboard('/same', 'copy', stamp)).rejects.toThrow('аудит');
});
