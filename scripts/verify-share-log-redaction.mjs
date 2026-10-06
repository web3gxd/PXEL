import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { createReadStream, statSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { join, resolve } from 'node:path';
import { createServer } from 'node:net';
import { test } from 'node:test';
import { connect, setCommand } from '../tests/helpers/client.mjs';

const root = process.cwd();
const databaseUrl = process.env.DATABASE_URL;
const keyringFile = process.env.PXEL_KEYRING_FILE;
const postgresLogPath = process.env.PXEL_TEST_POSTGRES_LOG;
assert.ok(databaseUrl, 'DATABASE_URL must point to an isolated acceptance database');
assert.ok(keyringFile, 'PXEL_KEYRING_FILE must point to the matching private test keyring');
const postgresLogOffset = postgresLogPath ? statSync(postgresLogPath).size : null;

function uuid7() {
  const bytes = randomBytes(16);
  bytes.writeUIntBE(Date.now(), 0, 6);
  bytes[6] = (bytes[6] & 0x0f) | 0x70;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = bytes.toString('hex');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

async function readPostgresLogDelta(path, startingOffset) {
  const size = statSync(path).size;
  const start = size >= startingOffset ? startingOffset : 0;
  const length = size - start;
  assert.ok(length <= 32 * 1024 * 1024, 'PostgreSQL log capture exceeded its bound');
  if (length === 0) return '';
  const chunks = [];
  for await (const chunk of createReadStream(path, { start, end: size - 1 })) chunks.push(chunk);
  return Buffer.concat(chunks, length).toString('utf8');
}

async function unusedLoopbackPort() {
  const server = createServer();
  await new Promise((resolveListen, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolveListen);
  });
  const { port } = server.address();
  await new Promise((resolveClose, reject) =>
    server.close((error) => (error ? reject(error) : resolveClose())),
  );
  return port;
}

async function waitReady(base, child) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline && child.exitCode === null && child.signalCode === null) {
    try {
      const response = await fetch(`${base}/readyz`, { signal: AbortSignal.timeout(750) });
      if (response.status === 200) return;
    } catch {
      /* Startup is bounded; the captured process output is checked on failure. */
    }
    await delay(100);
  }
  assert.fail(
    child.exitCode !== null || child.signalCode !== null
      ? 'test server exited before readiness'
      : 'test server readiness timed out',
  );
}

async function expectStatus(response, expected, label) {
  if (response.status === expected) return;
  let code = 'unknown';
  try {
    const body = await response.clone().json();
    code = body?.error?.code ?? body?.code ?? code;
  } catch {
    /* Keep diagnostics to the bounded public error category. */
  }
  throw new Error(`${label} returned status ${response.status} (code ${code})`);
}

async function verifyShareLogRedaction() {
  const targetDirectory = process.env.CARGO_TARGET_DIR
    ? resolve(process.env.CARGO_TARGET_DIR)
    : join(root, 'target');
  const binary = process.env.PXEL_TEST_SERVER_BIN
    ? resolve(process.env.PXEL_TEST_SERVER_BIN)
    : join(
        targetDirectory,
        'debug',
        process.platform === 'win32' ? 'pxel-server.exe' : 'pxel-server',
      );
  const port = await unusedLoopbackPort();
  const base = `http://127.0.0.1:${port}`;
  const origin = base;
  const child = spawn(binary, [], {
    cwd: root,
    windowsHide: true,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: {
      ...process.env,
      DATABASE_URL: databaseUrl,
      PXEL_KEYRING_FILE: keyringFile,
      PXEL_BIND: `127.0.0.1:${port}`,
      PXEL_ALLOWED_ORIGINS: `${origin},http://127.0.0.1:5173`,
      PXEL_DEV_AUTH: '1',
    },
  });
  let output = '';
  let outputBytes = 0;
  const maxOutputBytes = 8 * 1024 * 1024;
  let overflowed = false;
  const capture = (chunk) => {
    outputBytes += chunk.length;
    if (outputBytes <= maxOutputBytes) output += chunk.toString('utf8');
    else overflowed = true;
  };
  child.stdout.on('data', capture);
  child.stderr.on('data', capture);

  const markers = [];
  let exitError;
  let ownerCookie;
  let ownerSocket;
  try {
    await waitReady(base, child);

    const login = await fetch(`${base}/api/dev/session`, {
      method: 'POST',
      headers: { Origin: origin, 'X-PXEL-DEV-ACCOUNT': 'local' },
    });
    await expectStatus(login, 204, 'member login');
    ownerCookie = login.headers.get('set-cookie')?.split(';', 1)[0];
    assert.ok(ownerCookie, 'member login did not issue its session cookie');
    markers.push(ownerCookie);
    const ownerHeaders = {
      Origin: origin,
      Cookie: ownerCookie,
      'Content-Type': 'application/json',
    };

    const documentId = uuid7();
    const document = await fetch(`${base}/api/documents`, {
      method: 'POST',
      headers: ownerHeaders,
      body: JSON.stringify({ document_id: documentId, operation_id: uuid7() }),
    });
    await expectStatus(document, 201, 'document creation');
    const projection = await fetch(`${base}/api/documents/${documentId}`, {
      headers: ownerHeaders,
    });
    await expectStatus(projection, 200, 'document projection');
    const snapshot = await projection.json();
    const cellCanary = `pxel-cell-log-canary-${randomBytes(24).toString('hex')}`;
    markers.push(cellCanary);
    ownerSocket = await connect(base, ownerCookie);
    assert.equal(
      (await ownerSocket.send(setCommand(ownerSocket, snapshot, cellCanary))).case,
      'committed',
    );
    const current = await (
      await fetch(`${base}/api/documents/${documentId}`, { headers: ownerHeaders })
    ).json();
    const copy = await fetch(`${base}/api/documents/${documentId}/clipboard/authorize`, {
      method: 'POST',
      headers: ownerHeaders,
      body: JSON.stringify({
        schema_version: 1,
        action: 'copy',
        stamp: current.capabilities.disclosure_stamp,
      }),
    });
    await expectStatus(copy, 200, 'audited copy');
    const exportResponse = await fetch(`${base}/api/documents/${documentId}/export.xlsx`, {
      headers: ownerHeaders,
    });
    await expectStatus(exportResponse, 200, 'audited XLSX export');
    await exportResponse.arrayBuffer();

    const cases = [
      { kind: 'pin', pin: [...randomBytes(12)].map((byte) => byte % 10).join('') },
      { kind: 'password', password: `pxel-log-canary-${randomBytes(24).toString('hex')}` },
    ];
    for (const item of cases) {
      const secret = item.pin ?? item.password;
      markers.push(secret);
      const createShare = await fetch(`${base}/api/documents/${documentId}/shares`, {
        method: 'POST',
        headers: ownerHeaders,
        body: JSON.stringify({
          request_id: uuid7(),
          permission: 'view',
          ttl: 'day',
          confirm_indefinite: false,
          pin: item.pin ?? null,
          password: item.password ?? null,
        }),
      });
      await expectStatus(createShare, 201, `${item.kind} share creation`);
      const created = await createShare.json();
      assert.equal(typeof created.token, 'string');
      markers.push(created.token, created.url);

      const unlock = (candidate) =>
        fetch(`${base}/api/shared/${created.share.link_id}/session`, {
          method: 'POST',
          headers: { Origin: origin, 'Content-Type': 'application/json' },
          body: JSON.stringify({ token: created.token, secret: candidate }),
        });
      const wrongSecret = `wrong-${randomBytes(16).toString('hex')}`;
      markers.push(wrongSecret);
      const rejected = await unlock(wrongSecret);
      await expectStatus(rejected, 404, `${item.kind} wrong-secret attempt`);
      const accepted = await unlock(secret);
      await expectStatus(accepted, 200, `${item.kind} successful unlock`);
      const guestCookie = accepted.headers.get('set-cookie');
      assert.ok(guestCookie, `${item.kind} unlock did not issue a guest cookie`);
      markers.push(guestCookie);

      const revoked = await fetch(
        `${base}/api/documents/${documentId}/shares/${created.share.link_id}`,
        {
          method: 'DELETE',
          headers: ownerHeaders,
          body: JSON.stringify({ request_id: uuid7() }),
        },
      );
      await expectStatus(revoked, 204, `${item.kind} share revocation`);
    }
    const policy = await (
      await fetch(`${base}/api/documents/${documentId}/disclosure-policy`, {
        headers: ownerHeaders,
      })
    ).json();
    const policyResponse = await fetch(`${base}/api/documents/${documentId}/disclosure-policy`, {
      method: 'PUT',
      headers: ownerHeaders,
      body: JSON.stringify({
        schema_version: 1,
        expected_revision: policy.revision,
        rules: { ...policy.rules, allow_copy: false, export_formats: [] },
      }),
    });
    await expectStatus(policyResponse, 200, 'disclosure policy update');
    const deniedProjection = await (
      await fetch(`${base}/api/documents/${documentId}`, { headers: ownerHeaders })
    ).json();
    const deniedCopy = await fetch(`${base}/api/documents/${documentId}/clipboard/authorize`, {
      method: 'POST',
      headers: ownerHeaders,
      body: JSON.stringify({
        schema_version: 1,
        action: 'cut',
        stamp: deniedProjection.capabilities.disclosure_stamp,
      }),
    });
    await expectStatus(deniedCopy, 403, 'denied audited Cut');
    const audit = await fetch(`${base}/api/documents/${documentId}/audit`, {
      headers: ownerHeaders,
    });
    await expectStatus(audit, 200, 'owner disclosure audit');
    assert.ok(
      !(await audit.text()).includes(cellCanary),
      'cell content must not appear in disclosure audit',
    );
  } catch (error) {
    exitError = error;
  } finally {
    if (ownerCookie) {
      try {
        await fetch(`${base}/api/dev/session`, {
          method: 'DELETE',
          headers: { Origin: origin, Cookie: ownerCookie },
          signal: AbortSignal.timeout(1_000),
        });
      } catch {
        /* Expiry bounds the isolated acceptance session if cleanup is unavailable. */
      }
    }
    ownerSocket?.socket.terminate();
    if (child.exitCode === null && child.signalCode === null) {
      child.kill('SIGTERM');
      await Promise.race([
        new Promise((resolveClose) => child.once('close', resolveClose)),
        delay(5_000).then(() => {
          if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
        }),
      ]);
    }
  }

  if (overflowed) throw new Error('test server emitted more than the bounded log capture');
  if (exitError) throw exitError;
  assert.ok(
    output.includes('server_started'),
    'captured server output did not include its startup event',
  );
  for (const [index, marker] of markers.entries()) {
    if (marker && output.includes(marker)) {
      throw new Error(
        `sensitive value category ${index + 1} appeared in captured application logs`,
      );
    }
  }
  if (postgresLogPath && postgresLogOffset !== null) {
    const postgresOutput = await readPostgresLogDelta(postgresLogPath, postgresLogOffset);
    for (const [index, marker] of markers.entries()) {
      if (marker && postgresOutput.includes(marker)) {
        throw new Error(`sensitive value category ${index + 1} appeared in PostgreSQL logs`);
      }
    }
  }
  console.log(
    `share/auth log scan passed (application${postgresLogPath ? ' and PostgreSQL' : ''}; synthetic cell value and credentials absent)`,
  );
}

test(
  'share credentials and cell content are absent from application logs',
  verifyShareLogRedaction,
);
