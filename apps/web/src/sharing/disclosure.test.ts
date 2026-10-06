import { afterEach, expect, it, vi } from 'vitest';
import { decodePolicy, authorizeClipboard } from './disclosure';
afterEach(() => vi.unstubAllGlobals());
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
  expect(() => decodePolicy({ ...policy, schema_version: 2 })).toThrow();
  expect(() =>
    decodePolicy({ ...policy, rules: { ...policy.rules, export_formats: ['xlsx', 'xlsx'] } }),
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
