import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { join, resolve, sep } from 'node:path';
import { test } from 'node:test';
import {
  connect,
  createDocument,
  login,
  origin,
  releaseSessions,
  uuid7,
} from '../helpers/client.mjs';
import { freePort, killServer, startServer } from './private-postgres.mjs';

const root = resolve('.local');

test(
  'share revocation closes an idle WebSocket on a second server process',
  { timeout: 120000 },
  async () => {
    assert.ok(process.env.DATABASE_URL, 'DATABASE_URL must be the isolated acceptance database');
    assert.ok(process.env.PXEL_KEYRING_FILE, 'PXEL_KEYRING_FILE must match that database');
    mkdirSync(root, { recursive: true });
    const fixture = mkdtempSync(join(root, 'distributed-share-revoke-'));
    const firstPort = await freePort();
    const secondPort = await freePort();
    const firstBase = `http://127.0.0.1:${firstPort}`;
    const secondBase = `http://127.0.0.1:${secondPort}`;
    const sharedEnv = {
      ...process.env,
      PXEL_TEST_SERVER_BIN:
        process.env.PXEL_TEST_SERVER_BIN ??
        join(
          resolve(process.env.CARGO_TARGET_DIR ?? 'target'),
          'debug',
          process.platform === 'win32' ? 'pxel-server.exe' : 'pxel-server',
        ),
      PXEL_DEV_AUTH: '1',
      PXEL_ALLOWED_ORIGINS: `${origin},${firstBase},${secondBase}`,
    };
    const firstEnv = {
      ...sharedEnv,
      PXEL_BIND: `127.0.0.1:${firstPort}`,
      PXEL_BLOB_ROOT: join(fixture, 'first-blobs'),
    };
    const secondEnv = {
      ...sharedEnv,
      PXEL_BIND: `127.0.0.1:${secondPort}`,
      PXEL_BLOB_ROOT: join(fixture, 'second-blobs'),
    };
    let firstServer;
    let secondServer;
    let ownerCookie;
    let guestSocket;
    try {
      firstServer = await startServer(firstEnv, firstBase, join(fixture, 'first-server.log'));
      secondServer = await startServer(secondEnv, secondBase, join(fixture, 'second-server.log'));
      ownerCookie = await login(firstBase);
      const { snapshot } = await createDocument(firstBase, ownerCookie);
      const secret = [...randomBytes(8)].map((value) => String(value % 10)).join('');
      const createdResponse = await fetch(
        `${firstBase}/api/documents/${snapshot.document_id}/shares`,
        {
          method: 'POST',
          headers: {
            Origin: origin,
            Cookie: ownerCookie,
            'Content-Type': 'application/json',
          },
          body: JSON.stringify({
            request_id: uuid7(),
            permission: 'view',
            ttl: 'day',
            confirm_indefinite: false,
            pin: secret,
            password: null,
          }),
        },
      );
      assert.equal(createdResponse.status, 201);
      const created = await createdResponse.json();
      const link = created.share.link_id;
      const unlocked = await fetch(`${secondBase}/api/shared/${link}/session`, {
        method: 'POST',
        headers: { Origin: origin, 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: created.token, secret }),
      });
      assert.equal(unlocked.status, 200);
      const guestCookie = unlocked.headers.get('set-cookie')?.split(';', 1)[0];
      assert.ok(guestCookie);
      guestSocket = (await connect(secondBase, guestCookie, `/api/shared/${link}/ws`)).socket;

      const closed = new Promise((resolveClose, reject) => {
        const timer = setTimeout(
          () => reject(new Error('remote guest WebSocket stayed open')),
          5000,
        );
        guestSocket.once('close', (code) => {
          clearTimeout(timer);
          resolveClose(code);
        });
      });
      const revoked = await fetch(
        `${firstBase}/api/documents/${snapshot.document_id}/shares/${link}`,
        {
          method: 'DELETE',
          headers: {
            Origin: origin,
            Cookie: ownerCookie,
            'Content-Type': 'application/json',
          },
          body: JSON.stringify({ request_id: uuid7() }),
        },
      );
      assert.equal(revoked.status, 204);
      const closeCode = await closed;
      assert.equal(closeCode, 4003);

      const denied = await fetch(`${secondBase}/api/shared/${link}/document`, {
        headers: { Origin: origin, Cookie: guestCookie },
      });
      assert.equal(denied.status, 404);
    } finally {
      guestSocket?.terminate();
      if (ownerCookie) await releaseSessions(firstBase).catch(() => {});
      await killServer(secondServer);
      await killServer(firstServer);
      const safeFixture = resolve(fixture);
      if (!safeFixture.startsWith(root + sep))
        throw new Error('unsafe distributed-revocation fixture cleanup target');
      rmSync(safeFixture, { recursive: true, force: true });
    }
  },
);
