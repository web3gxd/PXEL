import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { cpus, platform, release } from 'node:os';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';

const require = createRequire(resolve('apps/web/package.json'));
const { chromium } = require('@playwright/test');
const base = process.env.PXEL_BENCH_WEB ?? 'http://127.0.0.1:8080';
const reloads = Number(process.env.PXEL_BENCH_LOADS ?? 20);
if (!Number.isInteger(reloads) || reloads < 1) throw new Error('PXEL_BENCH_LOADS must be positive');

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
  const source = await context.request.get(`/api/documents/${documentId}`);
  assert.equal(source.status(), 200);
  const projection = await source.json();
  const sheet = projection.sheets[0];
  while (sheet.rows.length < 10000) sheet.rows.push(uuid7());
  while (sheet.columns.length < 100) sheet.columns.push(uuid7());
  for (let row = 0; row < 10000; row++) {
    for (let offset = 0; offset < 10; offset++) {
      const column = (row * 7 + offset * 11) % 100;
      sheet.cells.push({
        row_id: sheet.rows[row],
        column_id: sheet.columns[column],
        value: { type: 'number', value: row * 100 + column },
      });
    }
  }
  assert.equal(sheet.cells.length, 100000);
  const body = JSON.stringify(projection);
  const page = await context.newPage();
  await page.route(`**/api/documents/${documentId}`, (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body }),
  );
  const collect = async () => {
    await page.waitForFunction(
      () => performance.getEntriesByName('pxel:document-grid-ready').length > 0,
      null,
      { timeout: 15000 },
    );
    return page.evaluate((id) => {
      const mark = performance.getEntriesByName('pxel:document-grid-ready')[0];
      const resource = performance
        .getEntriesByType('resource')
        .find((entry) => new URL(entry.name).pathname === `/api/documents/${id}`);
      const canvas = document.querySelector('canvas.grid-canvas');
      if (!mark || !resource || canvas?.dataset.documentReady !== id)
        throw new Error('Sparse document grid timing is incomplete');
      return {
        navigation_to_grid_ms: mark.startTime,
        data_to_grid_ms: mark.startTime - resource.responseEnd,
        dom_nodes: document.querySelectorAll('*').length,
        canvas_width: canvas.width,
        canvas_height: canvas.height,
      };
    }, documentId);
  };
  await page.goto(`/?document=${documentId}`, { waitUntil: 'load' });
  const first = await collect();
  const warm = [];
  for (let index = 0; index < reloads; index++) {
    await page.reload({ waitUntil: 'load' });
    warm.push(await collect());
  }
  assert.ok(warm.every((sample) => sample.data_to_grid_ms >= 0));
  assert.ok(Math.max(first.dom_nodes, ...warm.map((sample) => sample.dom_nodes)) < 1000);
  process.stdout.write(
    `${JSON.stringify(
      {
        kind: 'PXEL synthetic 100k-cell Canvas render',
        process: { platform: platform(), release: release(), cpu: cpus()[0]?.model },
        browser: await browser.version(),
        topology:
          'production bundle and release server on loopback; document GET intercepted with synthetic projection',
        dataset: {
          rows: 10000,
          columns: 100,
          filled_cells: 100000,
          response_bytes: Buffer.byteLength(body),
        },
        first,
        reloads,
        warm_navigation_to_grid_ms: percentiles(warm.map((sample) => sample.navigation_to_grid_ms)),
        warm_data_to_grid_ms: percentiles(warm.map((sample) => sample.data_to_grid_ms)),
        max_dom_nodes: Math.max(first.dom_nodes, ...warm.map((sample) => sample.dom_nodes)),
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
