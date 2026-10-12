import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve, sep } from 'node:path';
import { test } from 'node:test';
import { privatePath } from '../../scripts/repository-check.mjs';

test('publication check rejects local data, credentials and generated output', () => {
  for (const path of [
    '.local/keys/keyring.json',
    'apps/web/node_modules/x.js',
    'target/server',
    '.tools/gh/bin/gh.exe',
    'service/.env',
    'service/.env.production',
    'tls/server.key',
    'tls/certificate.pem',
    'database.backup',
    'capture.log',
    'test-results/state.json',
  ])
    assert.equal(privatePath(path), true, path);
  for (const path of [
    '.env.example',
    'fixtures/xlsx/minimal_scalars.xlsx',
    'proto/wire-descriptor.bin',
    'apps/web/src/auth/api.ts',
    'Cargo.lock',
    'pnpm-lock.yaml',
  ])
    assert.equal(privatePath(path), false, path);
});
test('bootstrap code is random, private, absent from output and never overwritten', () => {
  const root = resolve('.local');
  mkdirSync(root, { recursive: true });
  const fixture = mkdtempSync(join(root, 'bootstrap-test-'));
  assert.ok(fixture.startsWith(root + sep));
  try {
    const file = join(fixture, 'token');
    const output = execFileSync(process.execPath, ['scripts/bootstrap-token.mjs', file], {
      encoding: 'utf8',
      windowsHide: true,
    });
    const token = readFileSync(file, 'utf8').trim();
    assert.match(token, /^[0-9a-f]{64}$/);
    assert.equal(output.includes(token), false);
    const second = spawnSync(process.execPath, ['scripts/bootstrap-token.mjs', file], {
      encoding: 'utf8',
      windowsHide: true,
    });
    assert.equal(second.status, 1);
    assert.equal(readFileSync(file, 'utf8').trim(), token);
    assert.equal((second.stdout + second.stderr).includes(token), false);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});

test('infra bootstrap compiles from SQLx metadata before runtime migrates an empty database', () => {
  const local = resolve('.local');
  mkdirSync(local, { recursive: true });
  const fixture = mkdtempSync(join(local, 'infra-bootstrap-'));
  assert.ok(fixture.startsWith(local + sep));
  try {
    // Observe the real script's child-process boundary without Docker, Cargo,
    // or any developer database. An empty database cannot satisfy query! yet.
    const preload = join(fixture, 'observe.cjs');
    writeFileSync(
      preload,
      `const assert = require('node:assert/strict');
const childProcess = require('node:child_process');
const { writeFileSync } = require('node:fs');
const { syncBuiltinESMExports } = require('node:module');
let databaseReady = false;
childProcess.spawnSync = (command, args, options) => {
  if (command === 'docker') {
    assert.deepEqual(args, ['compose', 'up', '-d', '--wait', 'postgres']);
    databaseReady = true;
    return { status: 0 };
  }
  assert.equal(command, 'cargo');
  assert.deepEqual(args, ['run', '-p', 'pxel-server', '--', 'migrate']);
  assert.equal(databaseReady, true);
  assert.equal(options.env.SQLX_OFFLINE, 'true', 'bootstrap must not compile SQLx queries against the empty database');
  assert.match(options.env.DATABASE_URL, /^postgresql:\\/\\/pxel:[0-9a-f]+@127\\.0\\.0\\.1:55433\\/pxel$/);
  assert.equal(options.env.PXEL_KEYRING_JSON, 'fixture-not-read-by-cargo');
  writeFileSync('migration-invoked', 'offline compile; runtime database URL retained');
  return { status: 0 };
};
syncBuiltinESMExports();
`,
    );
    const result = spawnSync(
      process.execPath,
      ['--require', preload, resolve('scripts/infra.mjs')],
      {
        cwd: fixture,
        env: {
          ...process.env,
          SQLX_OFFLINE: 'false',
          PXEL_DB_PORT: '55433',
          PXEL_KEYRING_JSON: 'fixture-not-read-by-cargo',
        },
        encoding: 'utf8',
        windowsHide: true,
      },
    );
    assert.equal(result.status, 0, result.stderr);
    assert.equal(
      readFileSync(join(fixture, 'migration-invoked'), 'utf8'),
      'offline compile; runtime database URL retained',
    );
    const password = readFileSync(join(fixture, '.local/db-password'), 'utf8').trim();
    assert.equal((result.stdout + result.stderr).includes(password), false);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});
