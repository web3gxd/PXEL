import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  boundedJson,
  createShare,
  decodeCapabilities,
  decodeComments,
  decodeEncryption,
  decodeGuest,
  decodeShare,
  guestSession,
  revokeShare,
  SharingError,
  unlockShare,
} from './api';
import { shareRoute } from './SharedEntry';
const linkId = '019a1234-5678-789a-8bcd-0123456789ab';
const documentId = '019a1234-5678-789a-8bcd-0123456789ac';
const actorId = '019a1234-5678-789a-8bcd-0123456789ad';
const token = 'ab'.repeat(32);
const share = {
  link_id: linkId,
  document_id: documentId,
  permission: 'view',
  secret_kind: 'pin',
  status: 'active',
  created_at: 1791000000,
  expires_at: 1791086400,
};
const guest = {
  document_id: documentId,
  actor_id: actorId,
  permission: 'edit',
  expires_at: 1791086400,
  admitted_head_version: '42',
};
afterEach(() => vi.unstubAllGlobals());
it('CSV capabilities require version 3 and fail closed for older responses', () => {
  const caps = {
    schema_version: 3,
    can_edit: true,
    can_comment: true,
    can_share: true,
    can_export: true,
    can_export_pxel: true,
    can_export_csv: true,
    can_copy: true,
    anonymized: false,
    is_guest: false,
    disclosure_stamp: { document_version: 2, book_revision: 0, link_revision: 0 },
  };
  expect(decodeCapabilities(caps).can_export_csv).toBe(true);
  expect(() => decodeCapabilities({ ...caps, schema_version: 2 })).toThrow();
  expect(() =>
    decodeCapabilities({ ...caps, schema_version: 3, can_export_csv: undefined }),
  ).toThrow();
  const legacy = { ...caps } as Partial<typeof caps>;
  delete legacy.can_export_csv;
  expect(decodeCapabilities({ ...legacy, schema_version: 2 }).can_export_csv).toBeFalsy();
});
describe('sharing boundary', () => {
  it('recognizes shared routes before member login, including invalid links', () => {
    expect(shareRoute('/', '')).toBeUndefined();
    expect(shareRoute(`/share/${linkId}`, `#${token}`)).toEqual({ linkId, token });
    expect(shareRoute('/share/invalid', '#invalid')).toEqual({});
    expect(shareRoute(`/share/${linkId}/extra`, `#${token}`)).toEqual({ token });
  });
  it('keeps optional metadata strict, including the pre-rotation state', () => {
    expect(
      decodeEncryption({ encrypted: true, algorithm: 'XChaCha20-Poly1305', kek_rotated_at: null })
        .kek_rotated_at,
    ).toBeNull();
    expect(() =>
      decodeEncryption({ encrypted: false, algorithm: 'XChaCha20-Poly1305', kek_rotated_at: null }),
    ).toThrow();
    expect(() => decodeCapabilities({ can_edit: true })).toThrow();
    expect(() => decodeShare({ ...share, document_id: actorId }, documentId)).toThrow();
    expect(() => decodeShare({ ...share, permission: 'owner' }, documentId)).toThrow();
    expect(() =>
      decodeGuest({ ...guest, admitted_head_version: '9223372036854775808' }, linkId),
    ).toThrow();
  });
  it('bounds streamed JSON and rejects malformed or repeated comments', async () => {
    await expect(boundedJson(new Response('x'.repeat(1025)), 1024)).rejects.toBeInstanceOf(
      SharingError,
    );
    const comment = {
      comment_id: actorId,
      sheet_id: actorId,
      row_id: actorId,
      column_id: actorId,
      author_id: actorId,
      text: 'Комментарий',
      can_delete: true,
    };
    expect(decodeComments({ revision: '2', comments: [comment] }).comments).toHaveLength(1);
    expect(() => decodeComments({ revision: '2', comments: [comment, comment] })).toThrow();
    expect(() =>
      decodeComments({ revision: '2', comments: [{ ...comment, text: 'я'.repeat(2049) }] }),
    ).toThrow();
  });
  it('uses a scoped guest session endpoint and preserves exact secrets only in the request body', async () => {
    const fetch = vi.fn(
      async (_input: RequestInfo | URL, _init?: RequestInit) => new Response(JSON.stringify(guest)),
    );
    vi.stubGlobal('fetch', fetch);
    await guestSession(linkId);
    await unlockShare(linkId, token, '  Unicode 🔐 secret  ');
    expect(fetch.mock.calls[0]?.[0]).toBe(`/api/shared/${linkId}/session`);
    const init = fetch.mock.calls[1]?.[1] as RequestInit | undefined;
    expect(JSON.parse(String(init?.body))).toEqual({ token, secret: '  Unicode 🔐 secret  ' });
    expect(init?.credentials).toBe('same-origin');
    expect(fetch.mock.calls.every(([url]) => !String(url).includes(token))).toBe(true);
  });
  it('validates the one-time URL and accepts idempotent metadata-only replies', async () => {
    vi.stubGlobal('location', { origin: 'https://pxel.test' });
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ share, token, url: `/share/${linkId}#${token}` })),
      )
      .mockResolvedValueOnce(new Response(JSON.stringify({ share })))
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ share, token, url: `https://foreign.test/#${token}` })),
      );
    vi.stubGlobal('fetch', fetch);
    const body = {
      request_id: actorId,
      permission: 'view' as const,
      ttl: 'day' as const,
      confirm_indefinite: false,
      pin: '012345',
    };
    expect((await createShare(documentId, body)).url).toBe(
      `https://pxel.test/share/${linkId}#${token}`,
    );
    expect(await createShare(documentId, body)).toEqual({ share });
    await expect(createShare(documentId, body)).rejects.toThrow();
  });
  it('accepts no-content revocation and exposes only bounded lockouts', async () => {
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce(new Response(null, { status: 204 }))
        .mockResolvedValueOnce(
          new Response(JSON.stringify({ code: 'share_locked', retry_after_seconds: 60 }), {
            status: 429,
          }),
        )
        .mockResolvedValueOnce(new Response('do-not-echo-private-diagnostic', { status: 403 })),
    );
    await revokeShare(documentId, linkId, actorId);
    await expect(unlockShare(linkId, token, 'wrong')).rejects.toMatchObject({
      code: 'locked',
      retryAfter: 60,
    });
    await expect(unlockShare(linkId, token, 'wrong')).rejects.toMatchObject({
      code: 'unavailable',
    });
  });
});
