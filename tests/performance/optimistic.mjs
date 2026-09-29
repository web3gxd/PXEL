import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { cpus, platform, release } from 'node:os';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';

const require = createRequire(resolve('apps/web/package.json'));
const { chromium } = require('@playwright/test');
const base = process.env.PXEL_BENCH_WEB ?? 'http://127.0.0.1:8080';
const warmups = Number(process.env.PXEL_BENCH_WARMUPS ?? 20);
const samples = Number(process.env.PXEL_BENCH_SAMPLES ?? 100);
if (!Number.isInteger(warmups) || warmups < 0 || !Number.isInteger(samples) || samples < 1)
  throw new Error('PXEL_BENCH_WARMUPS and PXEL_BENCH_SAMPLES must be nonnegative integers');
if (warmups + samples > 128)
  throw new Error('Total commands must not exceed the client pending queue without ACK waits');

function uuid7() {
  const id = randomBytes(16);
  id.writeUIntBE(Date.now(), 0, 6);
  id[6] = (id[6] & 15) | 112;
  id[8] = (id[8] & 63) | 128;
  const hex = id.toString('hex');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function percentiles(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const at = (fraction) => sorted[Math.ceil(sorted.length * fraction) - 1];
  return { p50: at(0.5), p95: at(0.95), p99: at(0.99) };
}

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ baseURL: base });
try {
  const origin = new URL(base).origin;
  const session = await context.request.post('/api/dev/session', { headers: { Origin: origin } });
  assert.equal(session.status(), 204);
  const documentId = uuid7();
  const created = await context.request.post('/api/documents', {
    headers: { Origin: origin },
    data: { document_id: documentId, operation_id: uuid7() },
  });
  assert.equal(created.status(), 201, `Document create status: ${await created.text()}`);
  const page = await context.newPage();
  await page.addInitScript(() => {
    const samples = [];
    const probe = {
      armedValue: null,
      keydownAt: 0,
      samples,
      holdSocketSends: false,
      withheldSends: 0,
    };
    Object.defineProperty(window, '__pxelOptimisticProbe', { value: probe });
    const originalSend = WebSocket.prototype.send;
    WebSocket.prototype.send = function (data) {
      if (probe.holdSocketSends) {
        probe.withheldSends++;
        return;
      }
      return originalSend.call(this, data);
    };
    document.addEventListener(
      'keydown',
      (event) => {
        if (
          event.key === 'Enter' &&
          event.target instanceof HTMLInputElement &&
          event.target.classList.contains('formula-input') &&
          probe.armedValue !== null
        )
          probe.keydownAt = performance.now();
      },
      true,
    );
    const original = CanvasRenderingContext2D.prototype.fillText;
    CanvasRenderingContext2D.prototype.fillText = function (text, ...args) {
      const result = original.call(this, text, ...args);
      if (probe.keydownAt > 0 && String(text) === probe.armedValue) {
        samples.push({ value: probe.armedValue, elapsed_ms: performance.now() - probe.keydownAt });
        probe.armedValue = null;
        probe.keydownAt = 0;
      }
      return result;
    };
  });
  await page.goto(`/?document=${documentId}`, { waitUntil: 'load' });
  await page.getByTestId('connection-status').getByText('Подключено').waitFor();
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.waitFor();
  await page.evaluate(() => {
    window.__pxelOptimisticProbe.holdSocketSends = true;
  });
  const all = [];
  for (let index = 0; index < warmups + samples; index++) {
    const value = String(900000 + index);
    await bar.fill(value);
    await page.evaluate((next) => {
      window.__pxelOptimisticProbe.armedValue = next;
    }, value);
    await bar.press('Enter');
    await page.waitForFunction(
      (expected) =>
        window.__pxelOptimisticProbe.samples.some((sample) => sample.value === expected),
      value,
      { timeout: 10000 },
    );
    const elapsed = await page.evaluate(
      (expected) =>
        window.__pxelOptimisticProbe.samples.find((sample) => sample.value === expected)
          ?.elapsed_ms,
      value,
    );
    assert.equal(typeof elapsed, 'number');
    all.push(elapsed);
  }
  const measured = all.slice(warmups);
  const outcome = percentiles(measured);
  const withheldSends = await page.evaluate(() => window.__pxelOptimisticProbe.withheldSends);
  assert.ok(withheldSends > 0);
  const authoritative = await context.request.get(`/api/documents/${documentId}`);
  assert.equal(authoritative.status(), 200);
  assert.equal((await authoritative.json()).version, '1');
  process.stdout.write(
    `${JSON.stringify(
      {
        kind: 'PXEL optimistic Canvas draw submission after Enter',
        process: { platform: platform(), release: release(), cpu: cpus()[0]?.model },
        browser: await browser.version(),
        topology:
          'production bundle and release server on loopback; client WS sends withheld after subscription',
        dataset: 'one empty workbook; repeated numeric edits in A1',
        warmups,
        samples,
        withheld_socket_sends: withheldSends,
        confirmed_version_during_measurement: '1',
        ...outcome,
        max: Math.max(...measured),
      },
      null,
      2,
    )}\n`,
  );
} finally {
  await context.request
    .delete('/api/dev/session', { headers: { Origin: new URL(base).origin } })
    .catch(() => undefined);
  await context.close();
  await browser.close();
}
