import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve, sep } from 'node:path';
import { test } from 'node:test';

function smokeFixture(configured) {
  const local = resolve('.local');
  mkdirSync(local, { recursive: true });
  const fixture = mkdtempSync(join(local, 'smoke-environment-'));
  assert.ok(fixture.startsWith(local + sep));
  try {
    mkdirSync(join(fixture, '.local/keys'), { recursive: true });
    // Synthetic settings only: no database, Cargo, browser or server is started.
    const databaseUrl = 'postgresql://fixture:local-file@127.0.0.1:1/fixture';
    writeFileSync(join(fixture, '.local/database-url'), databaseUrl, { mode: 0o600 });
    writeFileSync(join(fixture, '.local/keys/keyring.json'), 'fixture-only-keyring', {
      mode: 0o600,
    });
    const playwrightPackage = join(fixture, 'apps/web/node_modules/@playwright/test');
    mkdirSync(playwrightPackage, { recursive: true });
    writeFileSync(join(playwrightPackage, 'package.json'), '{"name":"@playwright/test"}');
    const preload = join(fixture, 'observe.cjs');
    writeFileSync(
      preload,
      `const assert = require('node:assert/strict');
const childProcess = require('node:child_process');
const { EventEmitter } = require('node:events');
const { writeFileSync } = require('node:fs');
const { syncBuiltinESMExports } = require('node:module');
const { resolve } = require('node:path');
let devCount = 0, suiteCount = 0;
function checkEnvironment(options) {
  const env = options?.env ?? process.env;
  assert.ok(env.DATABASE_URL === process.env.FIXTURE_DATABASE_URL, 'every smoke child must receive the selected database URL');
  if (process.env.FIXTURE_CONFIGURED === '1') {
    assert.ok(env.PXEL_KEYRING_JSON === 'fixture-keyring-override', 'an explicit keyring must survive');
    assert.ok(!env.PXEL_KEYRING_FILE, 'do not add a conflicting keyring source');
  } else {
    assert.ok(env.PXEL_KEYRING_FILE === resolve('.local/keys/keyring.json'), 'every smoke child must receive the development keyring path');
  }
}
childProcess.spawn = (command, args, options) => {
  assert.equal(command, process.execPath);
  assert.deepEqual(args, ['scripts/dev.mjs']);
  checkEnvironment(options);
  devCount++;
  return new EventEmitter();
};
childProcess.spawnSync = (command, args, options) => {
  // Windows private-path ACL setup is unrelated to child env propagation.
  if (command === 'powershell.exe') return { status: 0 };
  assert.equal(command, process.execPath);
  checkEnvironment(options);
  suiteCount++;
  return { status: 0 };
};
global.fetch = async () => ({ ok: true });
process.on('exit', () => {
  assert.equal(devCount, 1);
  assert.equal(suiteCount, 3);
  writeFileSync('all-children-checked', 'passed');
});
syncBuiltinESMExports();
`,
    );
    const env = { ...process.env };
    for (const key of Object.keys(env))
      if (key.startsWith('PXEL_') || key === 'DATABASE_URL') delete env[key];
    env.FIXTURE_CONFIGURED = configured ? '1' : '0';
    env.FIXTURE_DATABASE_URL = configured
      ? 'postgresql://fixture:explicit-env@127.0.0.1:1/fixture'
      : databaseUrl;
    if (configured) {
      env.DATABASE_URL = env.FIXTURE_DATABASE_URL;
      env.PXEL_KEYRING_JSON = 'fixture-keyring-override';
    }
    const result = spawnSync(
      process.execPath,
      ['--require', preload, resolve('scripts/smoke.mjs')],
      { cwd: fixture, env, encoding: 'utf8', windowsHide: true, timeout: 10000 },
    );
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(join(fixture, 'all-children-checked'), 'utf8'), 'passed');
    assert.equal((result.stdout + result.stderr).includes(env.FIXTURE_DATABASE_URL), false);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
}

test('smoke passes local database and keyring to the dev stack and every test suite', () => {
  smokeFixture(false);
});

test('smoke preserves explicitly configured database and keyring sources for every child', () => {
  smokeFixture(true);
});
