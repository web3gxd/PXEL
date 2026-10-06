import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { cpus, platform, release } from 'node:os';

const require = createRequire(resolve('apps/web/package.json'));
const { chromium } = require('@playwright/test');
const id = (n) => `00000000-0000-7000-8000-${n.toString(16).padStart(12, '0')}`;
const rows = Array.from({ length: 100 }, (_, n) => id(100 + n));
const columns = Array.from({ length: 1000 }, (_, n) => id(1000 + n));
const sheet = id(2),
  document = id(1);
const address = (r, c) => ({ sheet_id: sheet, row_id: rows[r], column_id: columns[c] });
const cells = [];
for (let row = 0; row < 100; row++)
  for (let column = 0; column < 1000; column++) {
    cells.push({
      row_id: rows[row],
      column_id: columns[column],
      value:
        row === 99
          ? { type: 'formula', value: { source: '=A1+1' } }
          : { type: 'number', value: row * 1000 + column + 7 },
    });
  }
assert.equal(cells.length, 100_000);
const init = {
  schema_version: 1,
  document_id: document,
  input_version: '9',
  pending_revision: '0',
  sheets: [{ id: sheet, name: 'Main', rows, columns, cells }],
};
const initJson = JSON.stringify(init);
const initBytes = Buffer.byteLength(initJson);
assert.ok(initBytes <= 32 * 1024 * 1024, 'Keep the real 32MiB initialization cap');
const engine = readFileSync('apps/web/src/generated/wasm/spreadsheet_wasm.js');
assert.match(engine.toString(), /move_preview\(/, 'Build matching WASM before this benchmark');
const worker = `import init,{WasmWorkbook} from '/engine.js';
let book;const utf8=s=>new TextEncoder().encode(s).byteLength;
self.onmessage=async e=>{try{const request=e.data;
if(request.kind==='init'){const moduleStart=performance.now();await init({module_or_path:'/engine.wasm'});const moduleMs=performance.now()-moduleStart;const start=performance.now();book=new WasmWorkbook(request.json);const hydrated=performance.now();const raw=book.calculate();const calculated=performance.now();const output=JSON.parse(raw);const parsed=performance.now();self.postMessage({module_ms:moduleMs,hydrate_ms:hydrated-start,calculate_ms:calculated-hydrated,parse_ms:parsed-calculated,output,output_bytes:utf8(raw)});return;}
const encodeStart=performance.now();const json=JSON.stringify(request.command);const encoded=performance.now();const raw=book.move_preview(json);const called=performance.now();const output=JSON.parse(raw);const parsed=performance.now();
if(utf8(raw)>4*1024*1024)throw Error('real move response exceeds 4MiB');
self.postMessage({output,input_bytes:utf8(json),output_bytes:utf8(raw),encode_ms:encoded-encodeStart,call_ms:called-encoded,decode_ms:parsed-called,api_encode_call_decode_ms:parsed-encodeStart});
}catch(error){self.postMessage({error:String(error)});}};`;
const files = new Map([
  [
    '/',
    [
      'text/html',
      '<!doctype html><title>Large move preview benchmark</title><script src="/harness.js"></script>',
    ],
  ],
  [
    '/harness.js',
    [
      'text/javascript',
      `let worker;window.request=message=>new Promise((resolve,reject)=>{if(!worker)worker=new Worker('/worker.js',{type:'module'});const t=setTimeout(()=>{worker.terminate();worker=undefined;reject(Error('benchmark request timeout'));},120000);worker.onmessage=e=>{clearTimeout(t);e.data.error?reject(Error(e.data.error)):resolve(e.data);};worker.onerror=()=>{clearTimeout(t);reject(Error('benchmark worker failed'));};worker.postMessage(message);});window.closeWorker=()=>worker?.terminate();`,
    ],
  ],
  ['/worker.js', ['text/javascript', worker]],
  ['/engine.js', ['text/javascript', engine]],
  [
    '/engine.wasm',
    ['application/wasm', readFileSync('apps/web/src/generated/wasm/spreadsheet_wasm_bg.wasm')],
  ],
]);
let wasmLoads = 0;
const server = createServer((req, res) => {
  const file = files.get(req.url);
  if (!file) {
    res.writeHead(404);
    res.end();
    return;
  }
  if (req.url === '/engine.wasm') wasmLoads++;
  res.writeHead(200, {
    'Content-Type': file[0],
    'Content-Security-Policy':
      "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self'",
  });
  res.end(file[1]);
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
let browser;
const warmups = 5,
  measured = 20;
try {
  browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  // Page timing excludes Playwright protocol; it covers page->Worker->page delivery.
  const request = (message) =>
    page.evaluate(async (message) => {
      const start = performance.now();
      const result = await window.request(message);
      return { ...result, worker_roundtrip_ms: performance.now() - start };
    }, message);
  const initialized = await request({ kind: 'init', json: initJson });
  assert.equal(initialized.output.input_version, '9');
  assert.equal(initialized.output.pending_revision, '0');
  assert.equal(initialized.output.results.length, 1000);
  for (const item of initialized.output.results)
    assert.deepEqual(item.value, { type: 'number', value: 8 });
  const baselineHash = initialized.output.normalized_result_hash;
  const samples = [],
    warmupSamples = [];
  let revision = 0;
  const move = (source, destination, width) => ({
    schema_version: 1,
    document_id: document,
    input_version: '9',
    expected_pending_revision: String(revision),
    pending_revision: String(++revision),
    source,
    destination,
    rows: 1,
    columns: width,
  });
  for (let step = 0; step < warmups + measured; step++) {
    const forward = step % 2 === 0;
    const sourceRow = forward ? 0 : 1,
      destinationRow = forward ? 1 : 0;
    const result = await request({
      kind: 'move',
      command: move(address(sourceRow, 0), address(destinationRow, 0), 1000),
    });
    const output = result.output;
    assert.equal(output.preview, true);
    assert.equal(output.complete, true);
    assert.equal(output.input_version, '9');
    assert.equal(output.pending_revision, String(revision));
    assert.equal(output.formula_engine_version, 'pxel-formula-0.16.0');
    assert.equal(
      output.normalized_result_hash,
      baselineHash,
      'All 1000 dependent values remain 8 while references follow the cut',
    );
    assert.equal(output.move_writes.length, 3000);
    const cleared = output.move_writes.filter((w) => w.row_id === rows[sourceRow]);
    const moved = output.move_writes.filter((w) => w.row_id === rows[destinationRow]);
    const formulas = output.move_writes.filter((w) => w.row_id === rows[99]);
    assert.equal(cleared.length, 1000);
    assert.equal(moved.length, 1000);
    assert.equal(formulas.length, 1000);
    for (const item of cleared) assert.deepEqual(item.value, { type: 'blank' });
    const byColumn = new Map(moved.map((w) => [w.column_id, w.value]));
    for (let column = 0; column < 1000; column++)
      assert.deepEqual(byColumn.get(columns[column]), { type: 'number', value: column + 7 });
    for (const item of formulas)
      assert.deepEqual(item.value, {
        type: 'formula',
        value: { source: `=(A${destinationRow + 1}+1)` },
      });
    assert.equal(
      result.input_bytes,
      Buffer.byteLength(
        JSON.stringify({
          schema_version: 1,
          document_id: document,
          input_version: '9',
          expected_pending_revision: String(revision - 1),
          pending_revision: String(revision),
          source: address(sourceRow, 0),
          destination: address(destinationRow, 0),
          rows: 1,
          columns: 1000,
        }),
      ),
    );
    const { output: omitted, ...timing } = result;
    (step < warmups ? warmupSamples : samples).push(timing);
  }
  // Observe an independent scalar after all timed moves through the same real API.
  // This verification is outside the measured sample window.
  const independent = await request({
    kind: 'move',
    command: move(address(98, 999), address(97, 999), 1),
  });
  assert.deepEqual(independent.output.move_writes.find((w) => w.row_id === rows[97]).value, {
    type: 'number',
    value: 99006,
  });
  assert.equal(independent.output.normalized_result_hash, baselineHash);
  assert.equal(wasmLoads, 1);
  const distribution = (field) => {
    const sorted = samples.map((sample) => sample[field]).sort((a, b) => a - b);
    return {
      count: sorted.length,
      min: sorted[0],
      p50: sorted[Math.ceil(sorted.length * 0.5) - 1],
      p95: sorted[Math.ceil(sorted.length * 0.95) - 1],
      max: sorted.at(-1),
    };
  };
  const report = {
    schema_version: 1,
    engine_version: 'pxel-formula-0.16.0',
    initial_occupied_cells: cells.length,
    scalar_cells: 99000,
    dependent_formula_cells: 1000,
    source_cells: 1000,
    move_writes_per_sample: 3000,
    occupied_cells_after_first_overwrite: 99000,
    occupancy_note:
      'First move permanently overwrites 1000 destination scalars; subsequent alternating moves retain 99000 occupied cells.',
    warmups,
    measured_samples: measured,
    extra_independent_scalar_verification_moves: 1,
    input_version: '9',
    final_pending_revision: String(revision),
    initialization_input_utf8_bytes: initBytes,
    initialization_output_utf8_bytes: initialized.output_bytes,
    initialization: {
      wasm_module_ms: initialized.module_ms,
      hydrate_ms: initialized.hydrate_ms,
      calculate_ms: initialized.calculate_ms,
      decode_ms: initialized.parse_ms,
      worker_roundtrip_ms: initialized.worker_roundtrip_ms,
    },
    encode_ms: distribution('encode_ms'),
    wasm_move_call_ms: distribution('call_ms'),
    decode_ms: distribution('decode_ms'),
    api_encode_call_decode_ms: distribution('api_encode_call_decode_ms'),
    worker_roundtrip_ms: distribution('worker_roundtrip_ms'),
    measured_api_input_utf8_bytes: samples.reduce((sum, sample) => sum + sample.input_bytes, 0),
    measured_api_output_utf8_bytes: samples.reduce((sum, sample) => sum + sample.output_bytes, 0),
    samples,
    warmup_samples: warmupSamples,
    wasm_loads: wasmLoads,
    semantic_assertions_passed: true,
    memory:
      'not measured: portable process/WASM memory measurement API unavailable in this harness',
    concurrent_checks:
      process.env.PXEL_MOVE_BENCH_CONCURRENT_CHECKS ??
      'Root browser suite running concurrently; shared CPU, uncontrolled background load.',
    measurement_scope:
      'Single-machine sequential developer measurements, nearest-rank percentiles over 20 samples; no controlled production target or server durability claim. Worker roundtrip excludes Playwright protocol and includes output cloning and UTF8 byte counting outside API timing.',
    browser: browser.version(),
    os: `${platform()} ${release()}`,
    cpu: cpus()[0]?.model,
  };
  mkdirSync('.local', { recursive: true });
  writeFileSync('.local/move-preview-large.json', JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report, null, 2));
  await page.evaluate(() => window.closeWorker());
} finally {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
}
