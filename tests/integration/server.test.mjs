import assert from 'node:assert/strict';
import { once } from 'node:events';
import { request } from 'node:http';
import { test, afterEach } from 'node:test';
import WebSocket from 'ws';

const base = process.env.PXEL_TEST_SERVER ?? 'http://127.0.0.1:8080';
const origin = 'http://127.0.0.1:5173';
const socketUrl = base.replace(/^http/, 'ws') + '/api/ws';
const issuedCookies = [];
afterEach(async () => {
  for (const cookie of issuedCookies.splice(0)) {
    const response = await fetch(base + '/api/dev/session', {
      method: 'DELETE',
      headers: { Origin: origin, Cookie: cookie },
      signal: AbortSignal.timeout(5000),
    });
    assert.equal(response.status, 204);
  }
});
const hello = Buffer.from([
  8,
  1,
  18,
  28,
  10,
  16,
  ...Array(16).fill(1),
  18,
  8,
  ...Buffer.from('pxel-web'),
]);

async function cookie() {
  const response = await fetch(base + '/api/dev/session', {
    method: 'POST',
    headers: { Origin: origin },
  });
  assert.equal(response.status, 204);
  const value = response.headers.get('set-cookie');
  assert.match(value, /HttpOnly; SameSite=Strict; Path=\/api/);
  const cookie = value.split(';')[0];
  issuedCookies.push(cookie);
  return cookie;
}

async function rejectedUpgrade(headers, expected) {
  const socket = new WebSocket(socketUrl, { headers, handshakeTimeout: 3000 });
  await new Promise((resolve, reject) => {
    socket.on('unexpected-response', (_request, response) => {
      try {
        assert.equal(response.statusCode, expected);
        response.resume();
        socket.terminate();
        resolve();
      } catch (error) {
        reject(error);
      }
    });
    socket.on('open', () => {
      socket.terminate();
      reject(new Error('Unauthorized upgrade succeeded'));
    });
    socket.on('error', () => {});
    setTimeout(() => {
      socket.terminate();
      reject(new Error('Upgrade rejection timeout'));
    }, 4000).unref();
  });
}

test('health and readiness use real dependencies; security headers are present', async () => {
  for (const path of ['/healthz', '/readyz']) {
    const response = await fetch(base + path);
    assert.equal(response.status, 200);
    assert.equal(response.headers.get('x-content-type-options'), 'nosniff');
    assert.match(response.headers.get('content-security-policy'), /frame-ancestors 'none'/);
    const policy = response.headers.get('content-security-policy');
    assert.match(policy, /script-src 'self' 'wasm-unsafe-eval';/);
    assert.match(policy, /worker-src 'self';/);
    assert.equal(policy.includes("'unsafe-eval'"), false);
  }
});

test('development login enforces origin before issuing a cookie', async () => {
  for (const headers of [
    {},
    { Origin: 'https://evil.invalid' },
    { Origin: origin + '.evil.invalid' },
  ]) {
    const response = await fetch(base + '/api/dev/session', { method: 'POST', headers });
    assert.equal(response.status, 403);
    assert.equal(response.headers.get('set-cookie'), null);
  }
});

test('development logout revokes only its cookie and validates origin', async () => {
  const first = await cookie();
  const denied = await fetch(base + '/api/dev/session', {
    method: 'DELETE',
    headers: { Origin: 'https://evil.invalid', Cookie: first },
  });
  assert.equal(denied.status, 403);
  for (let repeat = 0; repeat < 2; repeat++) {
    const result = await fetch(base + '/api/dev/session', {
      method: 'DELETE',
      headers: { Origin: origin, Cookie: first },
    });
    assert.equal(result.status, 204);
    assert.match(result.headers.get('set-cookie'), /Max-Age=0/);
  }
  await rejectedUpgrade({ Origin: origin, Cookie: first }, 401);
});

test('websocket requires both an allowed origin and server-issued session', async () => {
  await rejectedUpgrade({ Origin: origin }, 401);
  await rejectedUpgrade({ Origin: 'https://evil.invalid', Cookie: await cookie() }, 403);
  await rejectedUpgrade({ Origin: origin, Cookie: 'pxel_dev_session=' + '0'.repeat(32) }, 401);
});

test('protobuf handshake succeeds and heartbeat receives a pong', async () => {
  const socket = new WebSocket(socketUrl, { headers: { Origin: origin, Cookie: await cookie() } });
  try {
    await once(socket, 'open');
    const response = once(socket, 'message');
    socket.send(hello);
    const [bytes, binary] = await response;
    assert.equal(binary, true);
    assert.deepEqual([...bytes.subarray(0, 3)], [8, 1, 18]); // ServerEnvelope.welcome
    const pong = once(socket, 'pong');
    socket.ping('check');
    const [payload] = await pong;
    assert.equal(payload.toString(), 'check');
  } finally {
    socket.terminate();
  }
});

test('unknown protocol produces typed error and closes without partial acceptance', async () => {
  const socket = new WebSocket(socketUrl, { headers: { Origin: origin, Cookie: await cookie() } });
  try {
    await once(socket, 'open');
    const response = once(socket, 'message');
    const closed = once(socket, 'close');
    const unknown = Buffer.from(hello);
    unknown[1] = 2;
    socket.send(unknown);
    const [bytes] = await response;
    assert.deepEqual([...bytes], [8, 1, 26, 4, 8, 1, 16, 1]);
    const [code] = await closed;
    assert.equal(code, 1002);
  } finally {
    socket.terminate();
  }
});

test('oversized websocket input closes the connection', async () => {
  const socket = new WebSocket(socketUrl, { headers: { Origin: origin, Cookie: await cookie() } });
  try {
    await once(socket, 'open');
    const closed = once(socket, 'close');
    socket.send(Buffer.alloc(2048));
    const [code] = await closed;
    assert.notEqual(code, 1000);
  } finally {
    socket.terminate();
  }
});

test('HTTP request body limit rejects an oversized command', async () => {
  // Expect/continue observes early 413 without racing an unread upload against
  // HTTP/1 connection shutdown (fetch can surface ECONNRESET during that race).
  const status = await new Promise((resolve, reject) => {
    const req = request(
      base + '/api/dev/session',
      {
        method: 'POST',
        headers: { Origin: origin, 'Content-Length': 1024 * 1024 + 1, Expect: '100-continue' },
      },
      (response) => {
        response.resume();
        resolve(response.statusCode);
      },
    );
    req.on('error', reject);
    req.on('continue', () => req.end(Buffer.alloc(1024 * 1024 + 1)));
    req.setTimeout(3000, () => req.destroy(new Error('HTTP limit response timeout')));
    req.flushHeaders();
  });
  assert.equal(status, 413);
});
