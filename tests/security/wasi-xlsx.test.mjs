import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { test } from 'node:test';

const runtime = resolve(
  process.env.PXEL_WASMTIME_BIN ??
    (process.platform === 'win32'
      ? '.tools/wasmtime/49.0.2/wasmtime-v49.0.2-x86_64-windows/wasmtime.exe'
      : '.tools/wasmtime/49.0.2/wasmtime-v49.0.2-x86_64-linux/wasmtime'),
);
const targetRoot = process.env.CARGO_TARGET_DIR ?? 'target';
const modulePath = resolve(targetRoot, 'wasm32-wasip1/release/pxel-xlsx-worker.wasm');
const settings = [
  'run',
  '-C',
  'cache=n',
  '-W',
  'fuel=500000000',
  '-W',
  'max-memory-size=268435456',
  '-W',
  'timeout=5s',
  '-S',
  'inherit-env=n',
  '-S',
  'inherit-network=n',
  '-S',
  'tcp=n',
  '-S',
  'udp=n',
  '-S',
  'http=n',
];
function request(source) {
  const header = Buffer.from('50584957000500000000', 'hex');
  header.writeUInt32BE(source.length, 6);
  return Buffer.concat([header, source]);
}
function run(input, module = modulePath, options = settings) {
  return spawnSync(runtime, [...options, module], {
    input,
    timeout: 15000,
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
    env: process.platform === 'win32' ? { SystemRoot: process.env.SystemRoot } : {},
  });
}
test('bounded WASI worker returns the complete existing two-sheet IR', () => {
  assert.match(
    spawnSync(runtime, ['--version'], { encoding: 'utf8' }).stdout,
    /^wasmtime 49\.0\.2 /,
  );
  const output = run(request(readFileSync('fixtures/xlsx/minimal_scalars.xlsx')));
  assert.equal(output.status, 0, 'WASI worker failed');
  assert.equal(output.stderr.length, 0);
  assert.equal(output.stdout.subarray(0, 7).toString('hex'), '50584f57000500');
  assert.equal(output.stdout.length, 11 + output.stdout.readUInt32BE(7));
  const book = JSON.parse(output.stdout.subarray(11));
  assert.equal(book.sheets.length, 2);
  assert.equal(
    book.sheets.reduce((n, sheet) => n + sheet.cells.length, 0),
    5,
  );
});

test('no filesystem preopen is granted; an explicit test mount changes the result', () => {
  const module = resolve('fixtures/wasi-sandbox/no-preopens.wat');
  assert.equal(run(Buffer.alloc(0), module).status, 0);
  assert.notEqual(
    run(Buffer.alloc(0), module, [...settings, '--dir', resolve('fixtures/wasi-sandbox')]).status,
    0,
  );
});
test('no environment is granted; an explicit synthetic variable changes the result', () => {
  const module = resolve('fixtures/wasi-sandbox/no-environment.wat');
  assert.equal(run(Buffer.alloc(0), module).status, 0);
  assert.notEqual(
    run(Buffer.alloc(0), module, [...settings, '--env', 'PXEL_SYNTHETIC_CANARY=present']).status,
    0,
  );
});
test('WASIp1 cannot acquire a new socket capability', () => {
  const output = run(Buffer.alloc(0), resolve('fixtures/wasi-sandbox/no-socket-open.wat'));
  assert.equal(output.status, 1);
  assert.match(output.stderr.toString(), /unknown import.*sock_open/);
  assert.equal(
    run(Buffer.alloc(0), resolve('fixtures/wasi-sandbox/no-inherited-socket.wat')).status,
    0,
  );
});
test('guest memory cannot grow beyond the cap; raising only the test cap changes the result', () => {
  const module = resolve('fixtures/wasi-sandbox/memory-limit.wat');
  assert.equal(run(Buffer.alloc(0), module).status, 0);
  assert.notEqual(
    run(
      Buffer.alloc(0),
      module,
      settings.map((v) => (v === 'max-memory-size=268435456' ? 'max-memory-size=536870912' : v)),
    ).status,
    0,
  );
});
test('fuel terminates an infinite loop', () => {
  const output = run(
    Buffer.alloc(0),
    resolve('fixtures/wasi-sandbox/infinite-loop.wat'),
    settings.map((v) => (v === 'fuel=500000000' ? 'fuel=10000' : v)),
  );
  assert.equal(output.status, 3);
  assert.match(output.stderr.toString(), /fuel/);
});
test('execution timeout terminates a loop independently of fuel exhaustion', () => {
  const output = run(
    Buffer.alloc(0),
    resolve('fixtures/wasi-sandbox/infinite-loop.wat'),
    settings.map((v) =>
      v === 'fuel=500000000' ? 'fuel=100000000000000' : v === 'timeout=5s' ? 'timeout=20ms' : v,
    ),
  );
  assert.equal(output.status, 3);
  assert.match(output.stderr.toString(), /interrupt|deadline|timeout/);
});
test('native and isolated WASI output match every committed XLSX profile fixture', () => {
  const native = resolve(
    targetRoot,
    'debug',
    process.platform === 'win32' ? 'pxel-server.exe' : 'pxel-server',
  );
  for (const file of [
    'minimal_scalars.xlsx',
    'single_sheet_scalars.xlsx',
    'single_sheet_merge.xlsx',
    'styled_profile.xlsx',
    'excel-profile1.xlsx',
    'excel-shared-formulas.xlsx',
    'cross_sheet_formula.xlsx',
  ]) {
    const input = request(readFileSync('fixtures/xlsx/' + file));
    const isolated = run(input);
    const existing = spawnSync(native, ['xlsx-worker'], {
      input,
      timeout: 15000,
      maxBuffer: 64 * 1024 * 1024,
      windowsHide: true,
    });
    assert.equal(isolated.status, 0, file + ': WASI execution failed');
    assert.equal(existing.status, 0, file + ': native execution failed');
    const checksum = (bytes) => createHash('sha256').update(bytes).digest('hex');
    assert.equal(
      checksum(isolated.stdout),
      checksum(existing.stdout),
      file + ': protocol/IR mismatch',
    );
  }
});

test('WASI rejects invalid request framing atomically', () => {
  for (const frame of [
    '50584957000400000003616263',
    '50584957000500000000',
    '50584957000502000001',
    '505849570005000000036162',
    '5058495700050000000361626364',
  ]) {
    const output = run(Buffer.from(frame, 'hex'));
    assert.equal(output.status, 1);
    assert.equal(output.stdout.length, 0);
    assert.equal(output.stderr.length, 0);
  }
});
test('WASI rejects a malformed package without echoing its source', () => {
  const output = run(request(Buffer.from('synthetic-private-cell-not-a-zip')));
  assert.equal(output.status, 0);
  assert.equal(output.stdout.toString('hex'), '50584f5700050100000000');
  assert.equal(output.stderr.length, 0);
});
