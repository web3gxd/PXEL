import assert from 'node:assert/strict';
import { cpus, platform } from 'node:os';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';

const require = createRequire(resolve('apps/web/package.json'));
const { chromium } = require('@playwright/test');
const base = process.env.PXEL_BENCH_WEB ?? 'http://127.0.0.1:5173';
const samples = Number(process.env.PXEL_BENCH_SAMPLES ?? 20);
assert.ok(Number.isInteger(samples) && samples > 0 && samples <= 100);
const browser = await chromium.launch();
const context = await browser.newContext({ baseURL: base });
try {
  assert.equal(
    (
      await context.request.post('/api/dev/session', {
        headers: { Origin: new URL(base).origin },
      })
    ).status(),
    204,
  );
  const page = await context.newPage();
  await page.addInitScript(() => {
    const probe = { expected: 0, inputAt: 0, samples: [] };
    Object.defineProperty(window, '__resizeProbe', { value: probe });
    window.addEventListener(
      'pointermove',
      () => {
        if (probe.expected) probe.inputAt = performance.now();
      },
      true,
    );
    const strokeRect = CanvasRenderingContext2D.prototype.strokeRect;
    CanvasRenderingContext2D.prototype.strokeRect = function (x, y, width, height) {
      const result = strokeRect.call(this, x, y, width, height);
      if (probe.inputAt && x === 47 && y === 29 && width === probe.expected - 2) {
        probe.samples.push(performance.now() - probe.inputAt);
        probe.expected = 0;
        probe.inputAt = 0;
      }
      return result;
    };
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Создать таблицу', exact: true }).click();
  await page.locator('canvas[data-document-ready]').waitFor();
  await page.getByTestId('save-status').filter({ hasText: 'Сохранено' }).waitFor();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  for (let index = 0; index < samples + 2; index++) {
    const from = index % 2 ? 172 : 112;
    const to = index % 2 ? 112 : 172;
    const grid = page.getByRole('region', { name: 'Область таблицы' });
    const box = await grid.boundingBox();
    await page.mouse.move(box.x + 46 + from, box.y + 14);
    await page.mouse.down();
    await page.evaluate((expected) => {
      window.__resizeProbe.expected = expected;
    }, to);
    await page.mouse.move(box.x + 46 + to, box.y + 14);
    await page.mouse.up();
    await page.waitForFunction(
      (length) => window.__resizeProbe.samples.length >= length,
      index + 1,
    );
    await page.getByTestId('save-status').filter({ hasText: 'Сохранено' }).waitFor();
  }
  const values = (await page.evaluate(() => window.__resizeProbe.samples))
    .slice(2)
    .sort((a, b) => a - b);
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify(
      {
        kind: 'pointermove to matching Canvas selection border draw submission',
        stage: process.env.PXEL_BENCH_STAGE ?? 'current',
        topology:
          process.env.PXEL_BENCH_TOPOLOGY ??
          'Vite dev frontend; dev-auth Rust server; real PostgreSQL; loopback',
        dataset: 'one empty durable workbook, alternating A-column 112/172 CSS pixels',
        platform: platform(),
        cpu: cpus()[0]?.model,
        browser: await browser.version(),
        samples,
        warmups: 2,
        p50_ms: values[Math.ceil(samples * 0.5) - 1],
        p95_ms: values[Math.ceil(samples * 0.95) - 1],
        max_ms: values.at(-1),
      },
      null,
      2,
    ),
  );
} finally {
  await context.request
    .delete('/api/dev/session', { headers: { Origin: new URL(base).origin } })
    .catch(() => undefined);
  await browser.close();
}
