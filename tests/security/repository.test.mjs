import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
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
