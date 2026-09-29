import { chromium } from '@playwright/test';
import { cpus, totalmem, release } from 'node:os';

const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  await page.goto(process.env.PXEL_BENCH_WEB ?? 'http://127.0.0.1:5173/', { waitUntil: 'load' });
  const timings = await page.evaluate(async () => {
    const run = () =>
      new Promise((resolve, reject) => {
        const start = performance.now();
        const worker = new Worker('/src/calculation/formula-copy.worker.ts', { type: 'module' });
        const timer = setTimeout(() => {
          worker.terminate();
          reject(new Error('copy timeout'));
        }, 10000);
        worker.onmessage = (event) => {
          clearTimeout(timer);
          worker.terminate();
          if (event.data.text !== '=(B2+$B$1)') reject(new Error('copy result mismatch'));
          else resolve(performance.now() - start);
        };
        worker.onerror = () => {
          clearTimeout(timer);
          worker.terminate();
          reject(new Error('worker error'));
        };
        worker.postMessage({
          source: '=A1+$B$1',
          fromRow: 0,
          fromColumn: 2,
          toRow: 1,
          toColumn: 3,
        });
      });
    for (let i = 0; i < 5; i++) await run();
    const samples = [];
    for (let i = 0; i < 50; i++) samples.push(await run());
    return samples.sort((a, b) => a - b);
  });
  const percentile = (p) => timings[Math.ceil(p * timings.length) - 1].toFixed(2);
  process.stdout.write(
    JSON.stringify({
      samples: timings.length,
      p50_ms: percentile(0.5),
      p95_ms: percentile(0.95),
      max_ms: timings.at(-1).toFixed(2),
      browser: browser.version(),
      os: `${process.platform} ${release()}`,
      cpu: cpus()[0]?.model,
      ram_gib: +(totalmem() / 2 ** 30).toFixed(1),
      scope: 'fresh Worker + cached WASM init + copy + JS reply, dev server',
    }) + '\n',
  );
} finally {
  await browser.close();
}
