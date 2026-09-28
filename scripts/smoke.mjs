import { spawn, spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { setTimeout } from 'node:timers/promises';

const webRequire = createRequire(resolve('apps/web/package.json'));
const playwright = join(dirname(webRequire.resolve('@playwright/test/package.json')), 'cli.js');
const dev = spawn(process.execPath, ['scripts/dev.mjs'], {
  stdio: 'inherit',
  windowsHide: true,
  detached: process.platform !== 'win32',
});
let exited = false;
dev.on('exit', () => {
  exited = true;
});

async function waitFor(url) {
  const deadline = Date.now() + 120000;
  while (Date.now() < deadline && !exited) {
    try {
      if ((await fetch(url, { signal: AbortSignal.timeout(2000) })).ok) return;
    } catch {
      /* Startup is bounded by the deadline; errors are not production fallbacks. */
    }
    await setTimeout(250);
  }
  throw new Error(`Stack did not become ready: ${url}`);
}

try {
  await waitFor('http://127.0.0.1:8080/readyz');
  await waitFor('http://127.0.0.1:5173');
  for (const args of [
    ['--test', 'tests/integration/server.test.mjs', 'tests/integration/document.test.mjs'],
    ['--test', 'tests/recovery/server-restart.test.mjs'],
    [playwright, 'test', '--config', 'apps/web/playwright.config.ts'],
  ]) {
    const result = spawnSync(process.execPath, args, { stdio: 'inherit', windowsHide: true });
    if (result.status !== 0) throw new Error('Live integration/browser checks failed');
  }
} finally {
  if (dev.pid) {
    if (process.platform === 'win32')
      spawnSync('taskkill', ['/pid', String(dev.pid), '/t', '/f'], {
        windowsHide: true,
        stdio: 'ignore',
      });
    else {
      try {
        dev.kill('SIGTERM');
      } catch (error) {
        if (error.code !== 'ESRCH') throw error;
      }
    }
  }
}
