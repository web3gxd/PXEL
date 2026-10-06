import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { cpus, platform, release } from 'node:os';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const require = createRequire(resolve(root, 'apps/web/package.json'));
const { chromium } = require('@playwright/test');
const corpus = JSON.parse(readFileSync(resolve(root, 'fixtures/formulas/profile-v1.json'), 'utf8'));
assert.equal(corpus.schema_version, 1);
const excelGolden = JSON.parse(
  readFileSync(resolve(root, 'fixtures/formulas/excel-criteria-2026-10-02.json'), 'utf8'),
);
assert.equal(excelGolden.verified_in_excel, true);
const excelNumberGolden = JSON.parse(
  readFileSync(resolve(root, 'fixtures/formulas/excel-number-precision-2026-10-02.json'), 'utf8'),
);
assert.equal(excelNumberGolden.verified_in_excel, true);
const excelCases = excelGolden.cases.map((test, index) => ({
  id: `excel-criteria-${index + 1}`,
  formula: test.formula,
  origin: 'Criteria!C1',
  expected: { type: 'number', value: test.result },
  cells: Object.fromEntries(
    excelGolden.workbook.cells.map((cell) => {
      let column = cell.column;
      let columnName = '';
      while (column > 0) {
        const remainder = (column - 1) % 26;
        columnName = String.fromCharCode(65 + remainder) + columnName;
        column = Math.floor((column - 1) / 26);
      }
      return [`Criteria!${columnName}${cell.row}`, { type: cell.type, value: cell.value }];
    }),
  ),
}));
const excelNumberCases = excelNumberGolden.cases.map((test, index) => ({
  id: `excel-number-precision-${index + 1}`,
  formula: test.formula,
  origin: 'Numbers!C1',
  expected:
    test.result_type === 'number'
      ? { type: 'number', value: test.result }
      : { type: 'error', value: test.result },
  ...(typeof test.tolerance === 'number' ? { tolerance: test.tolerance } : {}),
  cells: {},
}));
const allCases = [...corpus.cases, ...excelCases, ...excelNumberCases];
const requests = allCases.map((test) => {
  const origin = test.origin.slice(0, test.origin.lastIndexOf('!'));
  const sheets = new Set([origin]);
  const cells = Object.entries(test.cells).map(([address, value]) => {
    const index = address.lastIndexOf('!');
    const sheet = address.slice(0, index);
    const a1 = address.slice(index + 1);
    const match = /^([A-Z]+)([1-9][0-9]*)$/.exec(a1);
    assert.ok(match, 'Invalid fixture cell address');
    let column = 0;
    for (const character of match[1]) column = column * 26 + character.charCodeAt(0) - 64;
    sheets.add(sheet);
    return { sheet, row: Number(match[2]) - 1, column: column - 1, value };
  });
  return {
    source: test.formula,
    input: {
      schema_version: 1,
      current_sheet: origin,
      sheets: [...sheets],
      cells,
      ...(test.date_system === undefined ? {} : { date_system: test.date_system }),
    },
  };
});
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
    'fixture_runner',
  ],
  {
    cwd: root,
    input: JSON.stringify(requests),
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
    windowsHide: true,
  },
);
if (native.error || native.status !== 0)
  throw new Error(`Native fixture runner failed: ${native.stderr}`);
const nativeResults = JSON.parse(native.stdout);
const generated = resolve(root, 'apps/web/src/generated/wasm');
const files = new Map([
  [
    '/spreadsheet_wasm.js',
    ['text/javascript', readFileSync(resolve(generated, 'spreadsheet_wasm.js'))],
  ],
  [
    '/spreadsheet_wasm_bg.wasm',
    ['application/wasm', readFileSync(resolve(generated, 'spreadsheet_wasm_bg.wasm'))],
  ],
  [
    '/',
    [
      'text/html',
      '<!doctype html><title>PXEL WASM parity harness</title><script type="module" src="/harness.js"></script>',
    ],
  ],
  [
    '/harness.js',
    [
      'text/javascript',
      `window.runFixtures=(requests)=>new Promise((resolve,reject)=>{const worker=new Worker('/worker.js',{type:'module'});const timeout=setTimeout(()=>{worker.terminate();reject(new Error('Worker fixture timeout'));},30000);worker.onmessage=event=>{clearTimeout(timeout);worker.terminate();event.data.error?reject(new Error(event.data.error)):resolve(event.data);};worker.onerror=()=>{clearTimeout(timeout);worker.terminate();reject(new Error('Worker failed'));};worker.postMessage(requests);});`,
    ],
  ],
  [
    '/worker.js',
    [
      'text/javascript',
      `import init,{evaluate} from '/spreadsheet_wasm.js';self.onmessage=async event=>{try{const start=performance.now();await init({module_or_path:new URL('/spreadsheet_wasm_bg.wasm',self.location.href)});const initialized=performance.now();const results=event.data.map(item=>JSON.parse(evaluate(item.source,JSON.stringify(item.input))));self.postMessage({results,initialization_ms:initialized-start,evaluation_ms:performance.now()-initialized});}catch(error){self.postMessage({error:String(error)});}};`,
    ],
  ],
]);
let wasmLoads = 0,
  workerLoads = 0;
const server = createServer((request, response) => {
  const path = request.url?.split('?')[0];
  const file = files.get(path);
  if (!file) {
    response.writeHead(404);
    response.end();
    return;
  }
  if (path === '/spreadsheet_wasm_bg.wasm') wasmLoads++;
  if (path === '/worker.js') workerLoads++;
  response.writeHead(200, {
    'Content-Type': file[0],
    'Content-Security-Policy':
      "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self'; object-src 'none'",
  });
  response.end(file[1]);
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const address = server.address();
assert.ok(address && typeof address !== 'string');
let browser;
try {
  browser = await chromium.launch();
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto(`http://127.0.0.1:${address.port}`);
  await page.waitForFunction(() => typeof window.runFixtures === 'function');
  const started = performance.now();
  const result = await page.evaluate((requests) => window.runFixtures(requests), requests);
  const roundtrip = performance.now() - started;
  assert.equal(result.results.length, allCases.length);
  assert.equal(nativeResults.length, allCases.length);
  for (let i = 0; i < allCases.length; i++) {
    const test = allCases[i];
    if (typeof test.tolerance === 'number' && test.expected.type === 'number') {
      assert.equal(nativeResults[i].result.type, 'number', `Native result type: ${test.id}`);
      assert.ok(
        Math.abs(nativeResults[i].result.value - test.expected.value) <= test.tolerance,
        `Native expectation: ${test.id}`,
      );
    } else {
      assert.deepEqual(nativeResults[i].result, test.expected, `Native expectation: ${test.id}`);
    }
    assert.deepEqual(result.results[i], nativeResults[i], `WASM/native parity: ${test.id}`);
  }
  assert.equal(wasmLoads, 1);
  assert.equal(workerLoads, 1);
  assert.deepEqual(errors, []);
  await page.route('**/spreadsheet_wasm_bg.wasm', (route) => route.abort());
  const failed = await page.evaluate(
    async (requests) => {
      try {
        await window.runFixtures(requests);
        return false;
      } catch {
        return true;
      }
    },
    requests.slice(0, 1),
  );
  assert.equal(failed, true, 'Missing WASM must fail, not use a JavaScript evaluator');
  const report = {
    schema_version: 1,
    profile: corpus.profile,
    cases: allCases.length,
    excel_golden_cases: excelCases.length + excelNumberCases.length,
    excel_criteria_cases: excelCases.length,
    excel_number_precision_cases: excelNumberCases.length,
    wasm_loads: wasmLoads,
    worker_loads: workerLoads,
    native_wasm_parity: true,
    expected_values: true,
    missing_wasm_failure: true,
    initialization_ms: result.initialization_ms,
    evaluation_ms: result.evaluation_ms,
    roundtrip_ms: roundtrip,
    input_bytes: Buffer.byteLength(JSON.stringify(requests)),
    wasm_bytes: files.get('/spreadsheet_wasm_bg.wasm')[1].length,
    browser: browser.version(),
    os: `${platform()} ${release()}`,
    cpu: cpus()[0]?.model,
    node: process.version,
  };
  mkdirSync(resolve(root, '.local'), { recursive: true });
  writeFileSync(
    resolve(root, '.local/formula-parity.json'),
    JSON.stringify(report, null, 2) + '\n',
  );
  console.log(JSON.stringify(report, null, 2));
} finally {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
}
