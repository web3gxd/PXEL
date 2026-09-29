import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { cpus, platform, release } from 'node:os';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';

const require = createRequire(resolve('apps/web/package.json'));
const { chromium } = require('@playwright/test');
const base = process.env.PXEL_BENCH_WEB ?? 'http://127.0.0.1:8080';
const loads = Number(process.env.PXEL_BENCH_LOADS ?? 20);
if (!Number.isInteger(loads) || loads < 1) throw new Error('PXEL_BENCH_LOADS must be positive');

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
  assert.equal(created.status(), 201);
  const page = await context.newPage();
  const collect = async () => {
    await page.waitForFunction(
      () => performance.getEntriesByName('pxel:document-grid-ready').length > 0,
      null,
      { timeout: 10_000 },
    );
    return page.evaluate((id) => {
      const mark = performance.getEntriesByName('pxel:document-grid-ready')[0];
      const resource = performance
        .getEntriesByType('resource')
        .find((entry) => new URL(entry.name).pathname === `/api/documents/${id}`);
      const canvas = document.querySelector('canvas.grid-canvas');
      if (
        !mark ||
        !resource ||
        !(canvas instanceof HTMLCanvasElement) ||
        canvas.dataset.documentReady !== id
      )
        throw new Error('Document grid timing is incomplete');
      return {
        navigation_to_grid_ms: mark.startTime,
        data_to_grid_ms: mark.startTime - resource.responseEnd,
        dom_nodes: document.querySelectorAll('*').length,
        device_pixel_ratio: window.devicePixelRatio,
        viewport_width: window.innerWidth,
        viewport_height: window.innerHeight,
        canvas_width: canvas.width,
        canvas_height: canvas.height,
      };
    }, documentId);
  };
  await page.goto(`/?document=${documentId}`, { waitUntil: 'load' });
  const cold = await collect();
  const warm = [];
  for (let index = 0; index < loads; index++) {
    await page.reload({ waitUntil: 'load' });
    warm.push(await collect());
  }
  assert.ok(warm.every((sample) => sample.data_to_grid_ms >= 0));
  process.stdout.write(
    `${JSON.stringify(
      {
        kind: 'PXEL production Canvas document render',
        process: {
          platform: platform(),
          release: release(),
          cpu: cpus()[0]?.model,
          logical_cpus: cpus().length,
        },
        browser: await browser.version(),
        topology: {
          browser_server: 'loopback',
          assets: 'server-served production dist',
          server_build: process.env.PXEL_BENCH_SERVER_PROFILE ?? 'unverified',
        },
        loads: { cold: 1, warm: loads },
        cold,
        warm_navigation_to_grid_ms: percentiles(warm.map((sample) => sample.navigation_to_grid_ms)),
        warm_data_to_grid_ms: percentiles(warm.map((sample) => sample.data_to_grid_ms)),
        max_dom_nodes: Math.max(cold.dom_nodes, ...warm.map((sample) => sample.dom_nodes)),
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
