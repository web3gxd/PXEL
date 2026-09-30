import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { cpus, platform, release } from 'node:os';

const require = createRequire(resolve('apps/web/package.json'));
const { chromium } = require('@playwright/test');
const seed = 0x4d4f5645;
let random = seed;
const next = () => {
  random ^= random << 13;
  random ^= random >>> 17;
  random ^= random << 5;
  return random >>> 0;
};
const id = (n) => `00000000-0000-7000-8000-${n.toString(16).padStart(12, '0')}`;
const reference = (r, c) => `${String.fromCharCode(65 + c)}${r + 1}`;
const value = (type, value) => ({ type, value });
const formula = (source) => value('formula', { source });
const context = {
  schema_version: 1,
  context_id: id(9999),
  epoch: '1',
  effective_timestamp_utc_ms: '86400000',
  utc_offset_minutes: 180,
  civil_day_since_unix_epoch: 1,
  random_seed: '07'.repeat(32),
  engine_version: 'pxel-formula-0.14.0',
  compatibility_profile: 'pxel-excel-subset-v1',
  reason: 'explicit',
};
const requests = [],
  names = [],
  checks = [];
function fixture(name, cells, steps, options = {}) {
  const index = requests.length;
  const main = {
    id: id(1),
    name: 'Main',
    rows: Array.from({ length: 8 }, (_, n) => id(10 + n)),
    columns: Array.from({ length: 8 }, (_, n) => id(30 + n)),
    cells: [],
  };
  const other = {
    id: id(2),
    name: 'Other Sheet',
    rows: [id(50), id(51)],
    columns: [id(60)],
    cells: [],
  };
  const at = (r, c, sheet = main) => ({
    sheet_id: sheet.id,
    row_id: sheet.rows[r],
    column_id: sheet.columns[c],
  });
  for (const [r, c, content] of cells)
    main.cells.push({ row_id: main.rows[r], column_id: main.columns[c], value: content });
  if (options.other)
    for (const [r, source] of options.other) {
      other.cells.push({
        row_id: other.rows[r],
        column_id: other.columns[0],
        value: formula(source),
      });
    }
  const init = {
    schema_version: options.context ? 3 : 2,
    document_id: id(100 + index),
    input_version: '9',
    pending_revision: '0',
    sheets: options.other ? [main, other] : [main],
  };
  if (options.context) init.calculation_context = context;
  requests.push({
    init,
    moves: steps.map(([sr, sc, dr, dc, rows, columns], step) => ({
      schema_version: 1,
      document_id: init.document_id,
      input_version: '9',
      expected_pending_revision: String(step),
      pending_revision: String(step + 1),
      source: at(sr, sc),
      destination: at(dr, dc),
      rows,
      columns,
    })),
  });
  names.push(name);
  checks.push({ index, at, restore: options.restore ?? true, verify: options.verify });
}
fixture(
  'overlap-forward-follow-absolute-and-range',
  [
    [0, 0, value('number', 7)],
    [0, 1, value('number', 11)],
    [7, 7, formula('=SUM($B$1:A$1)')],
    [6, 7, formula('=$A$1')],
  ],
  [
    [0, 0, 0, 1, 1, 2],
    [0, 1, 0, 0, 1, 2],
  ],
  {
    verify(outputs, at) {
      const writes = outputs[1].move_writes;
      assert.deepEqual(find(writes, at(0, 0)).value, { type: 'blank' });
      assert.deepEqual(find(writes, at(0, 1)).value, value('number', 7));
      assert.deepEqual(find(writes, at(0, 2)).value, value('number', 11));
      assert.deepEqual(find(writes, at(7, 7)).value, formula('=SUM($C$1:B$1)'));
      assert.deepEqual(find(writes, at(6, 7)).value, formula('=$B$1'));
    },
  },
);
fixture(
  'overlap-reverse-frozen-text-and-formula',
  [
    [0, 1, value('text', 'Перенос 🧪')],
    [0, 2, formula('=G7+1')],
    [6, 6, value('number', 5)],
    [7, 7, formula('=C1')],
  ],
  [
    [0, 1, 0, 0, 1, 2],
    [0, 0, 0, 1, 1, 2],
  ],
  {
    verify(outputs, at) {
      assert.deepEqual(find(outputs[1].move_writes, at(0, 0)).value, value('text', 'Перенос 🧪'));
      assert.deepEqual(find(outputs[1].move_writes, at(0, 1)).value, formula('=(G7+1)'));
      assert.deepEqual(find(outputs[1].move_writes, at(7, 7)).value, formula('=B1'));
    },
  },
);
fixture(
  'three-consecutive-preview-moves',
  [
    [0, 0, value('number', 2)],
    [0, 1, value('boolean', true)],
    [7, 7, formula('=SUM(A1:B1)')],
  ],
  [
    [0, 0, 0, 1, 1, 2],
    [0, 1, 2, 3, 1, 2],
    [2, 3, 0, 0, 1, 2],
  ],
);
fixture(
  'blank-source-overwrites-destination-formula',
  [
    [0, 0, value('number', 7)],
    [0, 4, formula('=SUM(A1:C1)')],
  ],
  [[0, 0, 0, 3, 1, 2]],
  {
    restore: false,
    verify(outputs, at) {
      assert.deepEqual(find(outputs[1].move_writes, at(0, 4)).value, { type: 'blank' });
    },
  },
);
fixture(
  'cross-sheet-scalar-and-contained-range',
  [
    [0, 0, value('number', 3)],
    [0, 1, value('number', 4)],
  ],
  [
    [0, 0, 2, 2, 1, 2],
    [2, 2, 0, 0, 1, 2],
  ],
  {
    other: [
      [0, "='Main'!$A$1"],
      [1, "=SUM('Main'!B1:A1)"],
    ],
    verify(outputs) {
      const external = outputs[1].move_writes.filter((w) => w.sheet_id === id(2));
      assert.deepEqual(
        external.map((w) => w.value),
        [formula("='Main'!$C$3"), formula("=SUM('Main'!D3:C3)")],
      );
    },
  },
);
for (const source of ['=NOW()', '=TODAY()', '=RAND()', '=RAND()+NOW()']) {
  fixture(
    `persisted-context-${source}`,
    [[0, 0, formula(source)]],
    [
      [0, 0, 1, 1, 1, 1],
      [1, 1, 0, 0, 1, 1],
    ],
    {
      context: true,
      verify(outputs, at) {
        if (source === '=NOW()' || source === '=TODAY()') {
          const number = source === '=NOW()' ? 25570.125 : 25570;
          assert.deepEqual(find(outputs[0].results, at(0, 0)).value, value('number', number));
          assert.deepEqual(find(outputs[1].results, at(1, 1)).value, value('number', number));
        }
      },
    },
  );
}
for (let index = 0; index < 200; index++) {
  const sr = next() % 3,
    sc = next() % 3,
    rows = 1 + (next() % 3),
    columns = 1 + (next() % 3);
  let dr = next() % 5,
    dc = next() % 5;
  if (dr === sr && dc === sc) dc = (dc + 1) % 5;
  const cells = [];
  for (let r = sr; r < sr + rows; r++)
    for (let c = sc; c < sc + columns; c++) {
      const v = next();
      if (v % 5 !== 0)
        cells.push([
          r,
          c,
          v % 7 === 0 ? value('text', `текст-${v}`) : value('number', ((v % 2001) - 1000) / 8),
        ]);
    }
  // Each source includes a formula; its target and all observers stay outside both rectangles.
  const first = cells.findIndex(([r, c]) => r === sr && c === sc);
  if (first >= 0) cells.splice(first, 1);
  cells.push(
    [sr, sc, formula(index % 2 ? '=CONCAT("move",H1)' : '=H1*2')],
    [0, 7, value('number', 13)],
  );
  const range = `${reference(sr, sc)}:${reference(sr + rows - 1, sc + columns - 1)}`;
  cells.push(
    [7, 7, formula(`=SUM(${range})`)],
    [6, 7, formula(`=$${String.fromCharCode(65 + sc)}$${sr + 1}`)],
  );
  fixture(
    `seeded-${index}`,
    cells,
    [
      [sr, sc, dr, dc, rows, columns],
      [dr, dc, sr, sc, rows, columns],
    ],
    {
      other: [
        [0, `='Main'!${reference(sr, sc)}`],
        [1, `=SUM('Main'!${range})`],
      ],
      context: index % 4 === 0,
    },
  );
}
function find(items, address) {
  const found = items.find((item) =>
    ['sheet_id', 'row_id', 'column_id'].every((key) => item[key] === address[key]),
  );
  assert.ok(found, `Missing expected address ${JSON.stringify(address)}`);
  return found;
}
const input = JSON.stringify(requests);
const nativeStart = performance.now();
const native = spawnSync(
  'cargo',
  ['run', '--locked', '--quiet', '--release', '-p', 'spreadsheet-wasm', '--example', 'move_runner'],
  { input, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, windowsHide: true },
);
const nativeWall = performance.now() - nativeStart;
if (native.error || native.status !== 0)
  throw new Error(`Native move runner failed: ${native.error ?? native.stderr}`);
const expected = JSON.parse(native.stdout);
assert.equal(expected.length, requests.length);
for (const { index, at, restore, verify } of checks) {
  const outputs = expected[index];
  assert.equal(outputs.length, requests[index].moves.length + 1);
  for (const [step, output] of outputs.entries()) {
    assert.equal(output.preview, true);
    assert.equal(output.complete, true);
    assert.equal(output.input_version, '9');
    assert.equal(output.pending_revision, String(step));
    assert.equal(output.formula_engine_version, 'pxel-formula-0.14.0');
    assert.match(output.normalized_result_hash, /^[0-9a-f]{64}$/);
    if (step) assert.ok(output.move_writes.length >= 2);
  }
  if (restore)
    assert.equal(
      outputs.at(-1).normalized_result_hash,
      outputs[0].normalized_result_hash,
      `${names[index]} restores results`,
    );
  verify?.(outputs, at);
}
// Fail before opening a browser if root has not built the matching exported API.
const engineJs = readFileSync('apps/web/src/generated/wasm/spreadsheet_wasm.js');
assert.match(
  engineJs.toString(),
  /move_preview\(/,
  'Build matching WASM before running move parity',
);
const files = new Map([
  [
    '/',
    ['text/html', '<!doctype html><title>Move parity</title><script src="/harness.js"></script>'],
  ],
  [
    '/harness.js',
    [
      'text/javascript',
      `window.run=items=>new Promise((resolve,reject)=>{const w=new Worker('/worker.js',{type:'module'});const t=setTimeout(()=>{w.terminate();reject(Error('move parity timeout'));},120000);w.onmessage=e=>{clearTimeout(t);w.terminate();e.data.error?reject(Error(e.data.error)):resolve(e.data);};w.onerror=()=>{clearTimeout(t);w.terminate();reject(Error('move parity worker failed'));};w.postMessage(items);});`,
    ],
  ],
  [
    '/worker.js',
    [
      'text/javascript',
      `import init,{WasmWorkbook} from '/engine.js';self.onmessage=async e=>{try{await init({module_or_path:'/engine.wasm'});let apiInput=0,apiOutput=0;const bytes=s=>new TextEncoder().encode(s).byteLength;const start=performance.now();const results=e.data.map(item=>{const initial=JSON.stringify(item.init);apiInput+=bytes(initial);const book=new WasmWorkbook(initial);try{const first=book.calculate();apiOutput+=bytes(first);const outputs=[JSON.parse(first)];for(const move of item.moves){const command=JSON.stringify(move);apiInput+=bytes(command);const raw=book.move_preview(command);apiOutput+=bytes(raw);outputs.push(JSON.parse(raw));}return outputs;}finally{book.free();}});self.postMessage({results,compute_ms:performance.now()-start,api_input_utf8_bytes:apiInput,api_output_utf8_bytes:apiOutput});}catch(error){self.postMessage({error:String(error)});}};`,
    ],
  ],
  ['/engine.js', ['text/javascript', engineJs]],
  [
    '/engine.wasm',
    ['application/wasm', readFileSync('apps/web/src/generated/wasm/spreadsheet_wasm_bg.wasm')],
  ],
]);
let loads = 0;
const server = createServer((req, res) => {
  const file = files.get(req.url);
  if (!file) {
    res.writeHead(404);
    res.end();
    return;
  }
  if (req.url === '/engine.wasm') loads++;
  res.writeHead(200, {
    'Content-Type': file[0],
    'Content-Security-Policy':
      "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self'",
  });
  res.end(file[1]);
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
let browser;
try {
  browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  const start = performance.now();
  const actual = await page.evaluate((items) => window.run(items), requests);
  const roundtrip = performance.now() - start;
  assert.equal(actual.results.length, expected.length);
  for (let index = 0; index < expected.length; index++)
    assert.deepEqual(
      actual.results[index],
      expected[index],
      `Complete native/WASM outputs, hashes and rendered move_writes differ: ${names[index]}`,
    );
  assert.equal(loads, 1);
  assert.equal(
    actual.api_input_utf8_bytes,
    requests.reduce(
      (sum, item) =>
        sum +
        Buffer.byteLength(JSON.stringify(item.init)) +
        item.moves.reduce((n, move) => n + Buffer.byteLength(JSON.stringify(move)), 0),
      0,
    ),
  );
  const report = {
    schema_version: 1,
    seed,
    engine_version: 'pxel-formula-0.14.0',
    workbooks: requests.length,
    moves: requests.reduce((sum, item) => sum + item.moves.length, 0),
    native_wasm_parity: true,
    shared_expected_cases: checks.filter((c) => c.verify).length,
    wasm_loads: loads,
    corpus_input_bytes: Buffer.byteLength(input),
    native_stdout_bytes: Buffer.byteLength(native.stdout),
    api_input_utf8_bytes: actual.api_input_utf8_bytes,
    api_output_utf8_bytes: actual.api_output_utf8_bytes,
    native_cargo_run_wall_ms_includes_build: nativeWall,
    worker_compute_ms_includes_json_and_byte_counting: actual.compute_ms,
    worker_roundtrip_ms_includes_init_and_clone: roundtrip,
    measurement_scope:
      'One sequential developer corpus run; no percentiles or production latency claim.',
    browser: browser.version(),
    os: `${platform()} ${release()}`,
    cpu: cpus()[0]?.model,
  };
  mkdirSync('.local', { recursive: true });
  writeFileSync('.local/move-parity.json', JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report, null, 2));
} finally {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
}
