import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';

const target = process.env.CARGO_TARGET_DIR ?? 'target';
const binary = resolve(
  target,
  'debug',
  process.platform === 'win32' ? 'pxel-server.exe' : 'pxel-server',
);
const runtime = resolve(
  process.env.PXEL_WASMTIME_BIN ??
    (process.platform === 'win32'
      ? '.tools/wasmtime/49.0.2/wasmtime-v49.0.2-x86_64-windows/wasmtime.exe'
      : '.tools/wasmtime/49.0.2/wasmtime-v49.0.2-x86_64-linux/wasmtime'),
);
const modulePath = resolve(target, 'wasm32-wasip1/release/pxel-xlsx-worker.wasm');
const digest = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const manifest = {
  schema_version: 1,
  runtime_version: '49.0.2',
  runtime_path: runtime,
  runtime_sha256: digest(runtime),
  module_path: modulePath,
  module_sha256: digest(modulePath),
};
mkdirSync('.local', { recursive: true });
const root = mkdtempSync(resolve('.local/xlsx-runtime-'));
function preview(input, source = 'fixtures/xlsx/minimal_scalars.xlsx') {
  const file = join(root, 'manifest.json');
  writeFileSync(file, JSON.stringify(input));
  return spawnSync(binary, ['xlsx-preview', resolve(source)], {
    env: { ...process.env, PXEL_XLSX_SANDBOX_MANIFEST: file },
    encoding: 'utf8',
    timeout: 15000,
    windowsHide: true,
  });
}
test('configured WASI backend previews existing fixture without content output', () => {
  const output = preview(manifest);
  assert.equal(output.status, 0);
  assert.equal(output.stdout.trim(), 'XLSX preview: 2 sheets, 5 cells');
  assert.equal(output.stderr, '');
});

test('configured WASI backend rejects active package parts without publishing a preview', () => {
  for (const name of ['activex', 'control', 'macro-sheet', 'dialog-sheet']) {
    const output = preview(manifest, `fixtures/xlsx/active-content/${name}.xlsx`);
    assert.notEqual(output.status, 0);
    assert.equal(output.stdout, '');
    assert.match(output.stderr, /XLSX worker rejected the package/);
    assert.doesNotMatch(output.stderr, /payload|Budget|hello/);
  }
});
test('configured backend fails closed on bad pins, versions, paths and invented arguments', () => {
  for (const change of [
    { module_sha256: '0'.repeat(64) },
    { runtime_sha256: '0'.repeat(64) },
    { schema_version: 2 },
    { runtime_version: '49.0.1' },
    { module_sha256: 'invalid' },
    { runtime_path: join(root, 'missing-private-runtime-marker') },
    { module_path: 'relative-private-module-marker' },
    { args: ['--dir', '/'] },
  ]) {
    const output = preview({ ...manifest, ...change });
    assert.notEqual(output.status, 0, 'invalid sandbox manifest executed successfully');
    assert.equal(output.stdout, '');
    assert.doesNotMatch(output.stderr, /private-runtime-marker|private-module-marker|hello/);
  }
});

test('a correctly pinned failing WASI module cannot fall back to the native parser', () => {
  const emptyModule = join(root, 'empty.wasm');
  writeFileSync(emptyModule, Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]));
  const output = preview({
    ...manifest,
    module_path: emptyModule,
    module_sha256: digest(emptyModule),
  });
  assert.notEqual(output.status, 0);
  assert.equal(output.stdout, '');
  assert.doesNotMatch(output.stderr, /hello|empty\.wasm/);
});

test('configured backend bounds a looping WASI module without native fallback', () => {
  const loopingModule = join(root, 'loop.wasm');
  // Trusted synthetic core Wasm: exported _start loops indefinitely.
  writeFileSync(
    loopingModule,
    Buffer.from(
      '0061736d0100000001040160000003020100070a01065f737461727400000a0901070003400c000b0b',
      'hex',
    ),
  );
  const output = preview({
    ...manifest,
    module_path: loopingModule,
    module_sha256: digest(loopingModule),
  });
  assert.equal(output.error, undefined, 'parent execution exceeded the test deadline');
  assert.notEqual(output.status, 0);
  assert.equal(output.stdout, '');
  assert.match(output.stderr, /XLSX worker (protocol failed|exceeded its deadline)/);
  assert.doesNotMatch(output.stderr, /loop\.wasm|hello/);
});
