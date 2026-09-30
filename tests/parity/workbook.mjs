import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { cpus, platform, release } from 'node:os';

const require = createRequire(resolve('apps/web/package.json'));
const { chromium } = require('@playwright/test');
const seed = 0x5058454c;
let random = seed;
const next = () => {
  random ^= random << 13;
  random ^= random >>> 17;
  random ^= random << 5;
  return random >>> 0;
};
const id = (n) => `00000000-0000-7000-8000-${n.toString(16).padStart(12, '0')}`;
const requests = Array.from({ length: 1000 }, (_, index) => {
  const document = id(index + 100),
    sheet = id(1),
    row = id(2),
    columns = [id(3), id(4), id(5)];
  const number = () => ((next() % 2001) - 1000) / 8;
  const a = number(),
    b = number(),
    replacement = number();
  const sources = [
    '=A1+B1',
    '=SUM(A1:B1)',
    '=AVERAGE(A1:B1)',
    '=IF(A1>B1,A1,B1)',
    '=A1*B1',
    '=MIN(A1:B1)',
    '=MAX(A1:B1)',
    '=COUNT(A1:B1)',
    '=COUNTA(A1:B1)',
    '=IFS(A1>B1,A1,TRUE,B1)',
    '=IFERROR(A1/B1,0)',
    '=AND(A1>0,B1>0)',
    '=OR(A1:B1)',
    '=NOT(A1=B1)',
    '=ABS(A1)',
    '=MOD(A1,B1)',
    '=ROUND(A1,2)',
    '=ROUNDUP(A1,1)',
    '=ROUNDDOWN(A1,-1)',
    '=POWER(A1,B1)',
    '=ABS(A1)^0.5',
    '=CONCAT(A1:B1)',
    '=LEFT(CONCAT(A1),2)',
    '=RIGHT(CONCAT(A1),2)',
    '=MID(CONCAT(A1),2,2)',
    '=LEN(CONCAT(A1))',
    '=TRIM(CONCAT("  ",A1,"  "))',
    '=LOWER(CONCAT("ПРИВЕТ",A1))',
    '=UPPER(CONCAT("straße",A1))',
    '=DATE(2000,MOD(ABS(A1),12)+1,1)',
    '=YEAR(DATE(2000+ABS(A1),2,29))',
    '=MONTH(DATE(2000+ABS(A1),2,29))',
    '=DAY(DATE(2000+ABS(A1),2,29))',
    '=INDEX(A1:B1,1,2)',
    '=SUM(INDEX(A1:B1,1,0))',
    '=INDEX(INDEX(A1:B1,1,0),1,1)',
    '=MATCH(A1,A1:B1,0)',
    '=MATCH(B1,A1:B1,0)',
    '=VLOOKUP(A1,A1:B1,2,FALSE)',
    '=HLOOKUP(B1,A1:B1,1,FALSE)',
    '=XLOOKUP(A1,A1:B1,A1:B1)',
    '=SUM(XLOOKUP(B1,A1:B1,A1:B1,,0,-1))',
    '=XLOOKUP(A1,A1:B1,A1:B1,,0,2)',
    '=INDEX(A1:B1,1,MATCH(B1,A1:B1,0))',
    '=COUNTIF(A1:B1,">0")',
    '=COUNTIFS(A1:B1,">0",A1:B1,"<50")',
    '=SUMIFS(A1:B1,A1:B1,">0",A1:B1,"<50")',
    '=SUMIFS(INDEX(A1:B1,1,0),A1:B1,">0")',
    '=SUMIF(A1:B1,">0")',
    '=SUMIF(A1:B1,">0",A1)',
    '=SUMIF(A1:B1,">0",INDEX(A1:B1,1,0))',
  ];
  const source = sources[next() % sources.length];
  const primitive = (value) => ({ type: 'number', value });
  const init = {
    schema_version: 1,
    document_id: document,
    input_version: '3',
    pending_revision: '0',
    sheets: [
      {
        id: sheet,
        name: 'Sheet1',
        rows: [row],
        columns,
        cells: [
          { row_id: row, column_id: columns[0], value: primitive(a) },
          { row_id: row, column_id: columns[1], value: primitive(b) },
          { row_id: row, column_id: columns[2], value: { type: 'formula', value: { source } } },
        ],
      },
    ],
  };
  const mutation = (revision, value) => ({
    schema_version: 1,
    document_id: document,
    expected_input_version: '3',
    input_version: '3',
    expected_pending_revision: String(revision - 1),
    pending_revision: String(revision),
    cells: [{ sheet_id: sheet, row_id: row, column_id: columns[0], value: primitive(value) }],
  });
  return { init, patches: [mutation(1, replacement), mutation(2, a)] };
});
{
  const document = id(2000),
    first = id(2001),
    second = id(2002);
  const rowA = id(2003),
    rowB = id(2004),
    colA = id(2005),
    colB = id(2006);
  const patch = (revision, value) => ({
    schema_version: 1,
    document_id: document,
    expected_input_version: '3',
    input_version: '3',
    expected_pending_revision: String(revision - 1),
    pending_revision: String(revision),
    cells: [{ sheet_id: second, row_id: rowB, column_id: colB, value: { type: 'number', value } }],
  });
  requests.push({
    init: {
      schema_version: 2,
      document_id: document,
      input_version: '3',
      pending_revision: '0',
      sheets: [
        {
          id: first,
          name: 'Main',
          rows: [rowA],
          columns: [colA],
          cells: [
            {
              row_id: rowA,
              column_id: colA,
              value: { type: 'formula', value: { source: "='Other Sheet'!A1+1" } },
            },
          ],
        },
        {
          id: second,
          name: 'Other Sheet',
          rows: [rowB],
          columns: [colB],
          cells: [{ row_id: rowB, column_id: colB, value: { type: 'number', value: 4 } }],
        },
      ],
    },
    patches: [patch(1, 9), patch(2, 4)],
  });
}
// A persisted context is transported unchanged through native/browser hydration
// and ordinary patches. Current profile functions remain nonvolatile here.
const contextTemplate = {
  schema_version: 1,
  context_id: id(3000),
  epoch: '1',
  effective_timestamp_utc_ms: '86400000',
  utc_offset_minutes: 180,
  civil_day_since_unix_epoch: 1,
  random_seed: '01'.repeat(32),
  engine_version: 'pxel-formula-0.14.0',
  compatibility_profile: 'pxel-excel-subset-v1',
  reason: 'explicit',
};
for (const request of [requests[0], requests.at(-1)]) {
  requests.push({
    ...request,
    init: { ...request.init, schema_version: 3, calculation_context: contextTemplate },
  });
}
const volatileExpectations = [];
const repeatedId = (byte) => {
  const hex = byte.toString(16).padStart(2, '0').repeat(16);
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
};
for (const [source, value] of [
  ['=TODAY()', { type: 'number', value: 25570 }],
  ['=NOW()', { type: 'number', value: 25570.125 }],
  ['=RAND()', { type: 'number', value: 0.8435600626462038 }],
  ['=RAND()+A1*0', { type: 'number', value: 0.8435600626462038 }],
  ['=RAND()=RAND()', { type: 'boolean', value: false }],
  ['=IF(FALSE,RAND(),7)', { type: 'number', value: 7 }],
  ['=RAND(1)', { type: 'error', value: '#VALUE!' }],
]) {
  const document = repeatedId(3),
    sheet = repeatedId(1),
    row = repeatedId(2),
    a = repeatedId(4),
    b = repeatedId(5);
  const mutation = (revision, value) => ({
    schema_version: 1,
    document_id: document,
    expected_input_version: '3',
    input_version: '3',
    expected_pending_revision: String(revision - 1),
    pending_revision: String(revision),
    cells: [{ sheet_id: sheet, row_id: row, column_id: a, value: { type: 'number', value } }],
  });
  volatileExpectations.push({ index: requests.length, value, source });
  requests.push({
    init: {
      schema_version: 3,
      document_id: document,
      input_version: '3',
      pending_revision: '0',
      calculation_context: {
        ...contextTemplate,
        context_id: repeatedId(6),
        random_seed: '07'.repeat(32),
      },
      sheets: [
        {
          id: sheet,
          name: 'Sheet1',
          rows: [row],
          columns: [a, b],
          cells: [
            { row_id: row, column_id: a, value: { type: 'number', value: 0 } },
            { row_id: row, column_id: b, value: { type: 'formula', value: { source } } },
          ],
        },
      ],
    },
    patches: [mutation(1, 1), mutation(2, 0)],
  });
}
const input = JSON.stringify(requests);
const native = spawnSync(
  'cargo',
  [
    'run',
    '--locked',
    '--quiet',
    '--release',
    '-p',
    'spreadsheet-wasm',
    '--example',
    'stateful_runner',
  ],
  { input, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, windowsHide: true },
);
if (native.error || native.status !== 0)
  throw new Error(`Native stateful runner failed: ${native.stderr}`);
const expected = JSON.parse(native.stdout);
for (const fixture of volatileExpectations) {
  assert.deepEqual(
    expected[fixture.index][0].results[0].value,
    fixture.value,
    `Volatile expected fixture ${fixture.source}`,
  );
}
assert.equal(expected.length, requests.length);
for (const outputs of expected) {
  assert.equal(outputs.length, 3);
  assert.equal(
    outputs[0].normalized_result_hash,
    outputs[2].normalized_result_hash,
    'Reverting a preview delta restores the original normalized result hash',
  );
  for (const output of outputs) {
    assert.equal(output.preview, true);
    assert.equal(output.input_version, '3');
    assert.match(output.normalized_result_hash, /^[0-9a-f]{64}$/);
  }
}
const files = new Map([
  [
    '/',
    [
      'text/html',
      '<!doctype html><title>Stateful parity</title><script src="/harness.js"></script>',
    ],
  ],
  [
    '/harness.js',
    [
      'text/javascript',
      `window.run = requests => new Promise((resolve,reject)=>{const w=new Worker('/worker.js',{type:'module'});const t=setTimeout(()=>{w.terminate();reject(Error('timeout'));},60000);w.onmessage=e=>{clearTimeout(t);w.terminate();e.data.error?reject(Error(e.data.error)):resolve(e.data);};w.onerror=()=>{clearTimeout(t);w.terminate();reject(Error('worker failed'));};w.postMessage(requests);});`,
    ],
  ],
  [
    '/worker.js',
    [
      'text/javascript',
      `import init,{WasmWorkbook} from '/engine.js';self.onmessage=async e=>{try{await init({module_or_path:'/engine.wasm'});const start=performance.now();const results=e.data.map(item=>{const book=new WasmWorkbook(JSON.stringify(item.init));try{const outputs=[JSON.parse(book.calculate())];for(const patch of item.patches){book.apply(JSON.stringify(patch));outputs.push(JSON.parse(book.calculate()));}return outputs;}finally{book.free();}});self.postMessage({results,compute_ms:performance.now()-start});}catch{self.postMessage({error:'stateful worker failed'});}};`,
    ],
  ],
  [
    '/engine.js',
    ['text/javascript', readFileSync('apps/web/src/generated/wasm/spreadsheet_wasm.js')],
  ],
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
  assert.equal(actual.results.length, requests.length);
  for (let index = 0; index < expected.length; index++) {
    assert.deepEqual(
      actual.results[index],
      expected[index],
      `Stateful native/WASM results and hashes must match for workbook ${index}: native hashes ${expected[index].map((step) => step.normalized_result_hash)}, WASM hashes ${actual.results[index].map((step) => step.normalized_result_hash)}`,
    );
  }
  assert.equal(loads, 1);
  const report = {
    schema_version: 1,
    seed,
    workbooks: requests.length,
    patches: requests.reduce((total, request) => total + request.patches.length, 0),
    native_wasm_parity: true,
    wasm_loads: loads,
    input_bytes: Buffer.byteLength(input),
    worker_compute_ms: actual.compute_ms,
    roundtrip_ms: performance.now() - start,
    browser: browser.version(),
    os: `${platform()} ${release()}`,
    cpu: cpus()[0]?.model,
  };
  mkdirSync('.local', { recursive: true });
  writeFileSync('.local/workbook-parity.json', JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report, null, 2));
} finally {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
}
