import { spawn, spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { setTimeout } from 'node:timers/promises';
import { ensureDevelopmentKeyring } from './keyring.mjs';

const localDatabase = resolve('.local/database-url');
if (!process.env.DATABASE_URL && !existsSync(localDatabase))
  throw new Error('Run just infra-up (or just infra-native) first.');
// Runtime/recovery tests must use the same database and keys as the dev stack.
const env = { ...process.env, ...ensureDevelopmentKeyring() };
if (!env.DATABASE_URL) env.DATABASE_URL = readFileSync(localDatabase, 'utf8').trim();

const webRequire = createRequire(resolve('apps/web/package.json'));
const playwright = join(dirname(webRequire.resolve('@playwright/test/package.json')), 'cli.js');
const serverBinary = resolve(
  process.env.PXEL_TEST_SERVER_BINARY ??
    join(
      process.env.CARGO_TARGET_DIR ?? 'target',
      'debug',
      `pxel-server${process.platform === 'win32' ? '.exe' : ''}`,
    ),
);
const dev = spawn(process.execPath, ['scripts/dev.mjs'], {
  env,
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
    [
      '--test',
      '--test-concurrency=1',
      'tests/integration/server.test.mjs',
      'tests/integration/document.test.mjs',
      'tests/integration/xlsx-import.test.mjs',
      'tests/integration/network-convergence.test.mjs',
    ],
    [
      '--test',
      '--test-concurrency=1',
      'tests/recovery/server-restart.test.mjs',
      'tests/recovery/convergence-restart.test.mjs',
      'tests/recovery/disclosure-restart.test.mjs',
      'tests/recovery/views-restart.test.mjs',
      'tests/recovery/sort-restart.test.mjs',
      'tests/recovery/privacy-restart.test.mjs',
      'tests/recovery/history-restart.test.mjs',
      'tests/recovery/pxel-restart.test.mjs',
      'tests/recovery/library-restart.test.mjs',
    ],
    [playwright, 'test', '--config', 'apps/web/playwright.config.ts'],
  ]) {
    const result = spawnSync(process.execPath, args, {
      stdio: 'inherit',
      windowsHide: true,
      env: {
        ...env,
        PXEL_TEST_SERVER_BINARY: process.env.PXEL_TEST_SERVER_BINARY ?? serverBinary,
        PXEL_RECOVERY_SERVER_BINARY: process.env.PXEL_RECOVERY_SERVER_BINARY ?? serverBinary,
      },
    });
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
