import assert from 'node:assert/strict';
import { randomBytes, createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync, copyFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { dirname } from 'node:path';
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import { request as httpRequest } from 'node:http';
import { test } from 'node:test';
import { protectLocalPath } from '../../scripts/keyring.mjs';
import { exe, freePort, killServer, run, startServer } from './private-postgres.mjs';

test(
  'real XLSX preview is isolated, authorized, bounded and never publishes a book',
  { timeout: 180000 },
  async () => {
    assert.ok(process.env.PXEL_PG_BIN);
    mkdirSync('.local', { recursive: true });
    const root = mkdtempSync(resolve('.local/xlsx-preview-'));
    protectLocalPath(root, true);
    const password = randomBytes(24).toString('hex');
    const passwordFile = join(root, 'password');
    writeFileSync(passwordFile, password, { mode: 0o600 });
    protectLocalPath(passwordFile);
    const keyring = join(root, 'keyring.json');
    const key = randomBytes(32);
    try {
      writeFileSync(
        keyring,
        JSON.stringify({
          schema_version: 1,
          active_kek_id: 'xlsx-test',
          keys: [{ id: 'xlsx-test', key_hex: key.toString('hex') }],
        }),
        { mode: 0o600 },
      );
    } finally {
      key.fill(0);
    }
    protectLocalPath(keyring);
    const runtime = resolve(
      process.env.PXEL_WASMTIME_BIN ??
        (process.platform === 'win32'
          ? '.tools/wasmtime/49.0.2/wasmtime-v49.0.2-x86_64-windows/wasmtime.exe'
          : '.tools/wasmtime/49.0.2/wasmtime-v49.0.2-x86_64-linux/wasmtime'),
    );
    const module = resolve(
      process.env.CARGO_TARGET_DIR ?? 'target',
      'wasm32-wasip1/release/pxel-xlsx-worker.wasm',
    );
    const digest = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
    const manifest = join(root, 'sandbox.json');
    writeFileSync(
      manifest,
      JSON.stringify({
        schema_version: 1,
        runtime_version: '49.0.2',
        runtime_path: runtime,
        runtime_sha256: digest(runtime),
        module_path: module,
        module_sha256: digest(module),
      }),
    );
    const pgPort = await freePort(),
      appPort = await freePort(),
      webPort = await freePort();
    const base = `http://127.0.0.1:${appPort}`,
      origin = `http://127.0.0.1:${webPort}`;
    const data = join(root, 'data');
    const binary = join(root, process.platform === 'win32' ? 'server.exe' : 'server');
    copyFileSync(
      resolve(
        process.env.CARGO_TARGET_DIR ?? 'target',
        'debug',
        process.platform === 'win32' ? 'pxel-server.exe' : 'pxel-server',
      ),
      binary,
    );
    const env = {
      ...process.env,
      DATABASE_URL: `postgresql://pxel:${password}@127.0.0.1:${pgPort}/pxel`,
      PGPASSWORD: password,
      PXEL_DEV_AUTH: '1',
      PXEL_BIND: `127.0.0.1:${appPort}`,
      PXEL_ALLOWED_ORIGINS: origin,
      PXEL_KEYRING_FILE: keyring,
      PXEL_BLOB_ROOT: join(root, 'blobs'),
      PXEL_XLSX_SANDBOX_MANIFEST: manifest,
      PXEL_TEST_SERVER_BIN: binary,
    };
    delete env.PXEL_KEYRING_JSON;
    let server,
      web,
      browser,
      pgRunning = false,
      cookie;
    const call = (body, session = cookie, requestOrigin = origin) =>
      fetch(base + '/api/imports/xlsx/preview', {
        method: 'POST',
        headers: {
          Origin: requestOrigin,
          'Content-Type': 'application/json',
          ...(session ? { Cookie: session } : {}),
        },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(30000),
      });
    try {
      run(
        exe('initdb'),
        [
          '-D',
          data,
          '-U',
          'pxel',
          '--encoding=UTF8',
          '--locale=C',
          '--auth-host=scram-sha-256',
          '--auth-local=scram-sha-256',
          `--pwfile=${passwordFile}`,
        ],
        env,
      );
      run(
        exe('pg_ctl'),
        [
          '-D',
          data,
          '-l',
          join(root, 'postgres.log'),
          '-o',
          `-h 127.0.0.1 -p ${pgPort} -c synchronous_commit=on -c fsync=on -c full_page_writes=on`,
          '-w',
          'start',
        ],
        env,
      );
      pgRunning = true;
      run(exe('createdb'), ['-h', '127.0.0.1', '-p', String(pgPort), '-U', 'pxel', 'pxel'], env);
      server = await startServer(env, base, join(root, 'server.log'));
      const login = await fetch(base + '/api/dev/session', {
        method: 'POST',
        headers: { Origin: origin, 'X-PXEL-DEV-ACCOUNT': 'xlsx_' + randomBytes(8).toString('hex') },
      });
      assert.equal(login.status, 204);
      cookie = login.headers.get('set-cookie').split(';')[0];
      const input = {
        schema_version: 1,
        file_base64: readFileSync('fixtures/xlsx/minimal_scalars.xlsx').toString('base64'),
      };
      const anonymous = await fetch(base + '/api/imports/xlsx/preview', {
        method: 'POST',
        headers: { Origin: origin, 'Content-Type': 'application/json' },
        body: '{',
      });
      assert.equal(anonymous.status, 401);
      assert.equal((await call(input, cookie, 'https://foreign.invalid')).status, 403);
      const result = await call(input);
      assert.equal(result.status, 200);
      const preview = await result.json();
      assert.deepEqual(preview.selected_sheets, [0, 1]);
      assert.equal(
        preview.sheets.reduce((sum, sheet) => sum + sheet.cells, 0),
        5,
      );
      assert.equal(preview.preview_digest.length, 64);
      const selected = await call({ ...input, selected_sheets: [1] });
      assert.equal(selected.status, 200);
      assert.notEqual((await selected.json()).preview_digest, preview.preview_digest);
      for (const extra of [
        { schema_version: 2 },
        { selected_sheets: [] },
        { selected_sheets: [0, 0] },
        { selected_sheets: [2] },
        { file_base64: '!' },
      ]) {
        assert.equal((await call({ ...input, ...extra })).status, 400);
      }
      assert.equal((await call({ ...input, runtime_path: 'private-marker' })).status, 422);
      assert.equal((await call({ ...input, file_base64: '' })).status, 413);
      // Hold an authorized upload before EOF: other imports must not start.
      const slow = httpRequest(base + '/api/imports/xlsx/preview', {
        method: 'POST',
        headers: { Origin: origin, Cookie: cookie, 'Content-Type': 'application/json' },
      });
      slow.on('error', () => {});
      let slowStatus;
      slow.on('response', (response) => {
        slowStatus = response.statusCode;
        response.resume();
      });
      slow.write('{"schema_version":1,"file_base64":"');
      await new Promise((resolve) => setTimeout(resolve, 500));
      let held = false;
      for (let attempt = 0; attempt < 30; attempt++) {
        const response = await call(input);
        await response.arrayBuffer();
        if (response.status === 429) {
          held = true;
          break;
        }
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assert.ok(
        held,
        `a pending authorized body owns the import permit (early status ${slowStatus ?? 'none'})`,
      );
      slow.destroy();
      let released = false;
      for (let attempt = 0; attempt < 30; attempt++) {
        const response = await call(input);
        await response.arrayBuffer();
        if (response.status === 200) {
          released = true;
          break;
        }
        assert.equal(response.status, 429);
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assert.ok(released, 'cancelled body releases the import permit');
      const active = await call({
        ...input,
        file_base64: readFileSync('fixtures/xlsx/active-content/activex.xlsx').toString('base64'),
      });
      assert.equal(active.status, 422);
      const catalog = await fetch(base + '/api/documents?schema_version=1', {
        headers: { Cookie: cookie },
      });
      assert.equal(catalog.status, 200);
      assert.equal((await catalog.json()).documents.length, 0);
      const requireWeb = createRequire(resolve('apps/web/package.json'));
      const vite = join(dirname(requireWeb.resolve('vite/package.json')), 'bin/vite.js');
      web = spawn(process.execPath, [vite, '--host', '127.0.0.1'], {
        cwd: resolve('apps/web'),
        windowsHide: true,
        stdio: 'ignore',
        env: { ...env, PXEL_API_TARGET: base, PXEL_WEB_PORT: String(webPort) },
      });
      let webReady = false;
      for (let attempt = 0; attempt < 100; attempt++) {
        if (web.exitCode !== null) throw new Error('Private web fixture exited');
        const response = await fetch(origin, { signal: AbortSignal.timeout(300) }).catch(
          () => undefined,
        );
        if (response?.ok) {
          webReady = true;
          break;
        }
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assert.ok(webReady);
      const { chromium } = requireWeb('@playwright/test');
      browser = await chromium.launch();
      for (const [width, dpr] of [
        [390, 1],
        [1440, 2],
      ]) {
        const context = await browser.newContext({
          viewport: { width, height: 900 },
          deviceScaleFactor: dpr,
        });
        const page = await context.newPage();
        const errors = [];
        page.on('pageerror', (error) => errors.push(error.message));
        await page.goto(origin);
        await page.getByRole('button', { name: 'Предпросмотр XLSX', exact: true }).click();
        const dialog = page.getByRole('dialog', { name: 'Предпросмотр XLSX' });
        await dialog
          .getByLabel('XLSX-файл')
          .setInputFiles(resolve('fixtures/xlsx/minimal_scalars.xlsx'));
        await dialog.getByRole('button', { name: 'Предпросмотр', exact: true }).click();
        await dialog.getByRole('status').filter({ hasText: 'Проверено листов: 2.' }).waitFor();
        assert.equal(
          await dialog.getByRole('table', { name: 'Предпросмотр ячеек XLSX' }).count(),
          1,
        );
        await dialog.getByRole('combobox', { name: 'Показать лист' }).click();
        await page.getByRole('option', { name: preview.sheets[1].name, exact: true }).click();
        assert.equal(await dialog.getByRole('cell', { name: 'world!', exact: true }).count(), 1);
        await dialog.getByRole('combobox', { name: 'Показать лист' }).click();
        await page.getByRole('option', { name: preview.sheets[0].name, exact: true }).click();
        await dialog.getByLabel(preview.sheets[0].name, { exact: true }).uncheck();
        await dialog.getByRole('button', { name: 'Проверить выбранные листы' }).click();
        await dialog.getByRole('status').filter({ hasText: 'Проверено листов: 1.' }).waitFor();
        await dialog.getByLabel(preview.sheets[1].name, { exact: true }).uncheck();
        assert.equal(
          await dialog.getByRole('button', { name: 'Проверить выбранные листы' }).isDisabled(),
          true,
        );
        await dialog
          .locator('input[type="checkbox"]')
          .evaluateAll((elements) =>
            Promise.all(
              elements.flatMap((element) =>
                element.getAnimations().map((animation) => animation.finished),
              ),
            ),
          );
        await page.screenshot({ path: join(root, `preview-${width}-${dpr}.png`), fullPage: true });
        await page.keyboard.press('Escape');
        await dialog.waitFor({ state: 'hidden' });
        assert.equal(
          await page
            .getByRole('button', { name: 'Предпросмотр XLSX', exact: true })
            .evaluate((element) => element === document.activeElement),
          true,
        );
        await page.getByRole('button', { name: 'Предпросмотр XLSX', exact: true }).click();
        await dialog
          .getByLabel('XLSX-файл')
          .setInputFiles(resolve('fixtures/xlsx/active-content/activex.xlsx'));
        await dialog.getByRole('button', { name: 'Предпросмотр', exact: true }).click();
        await dialog.getByRole('alert').filter({ hasText: 'Файл повреждён' }).waitFor();
        assert.deepEqual(errors, []);
        await context.close();
      }
      await browser.close();
      browser = undefined;
      await killServer(web);
      web = undefined;
      await killServer(server);
      server = undefined;
      const disabled = { ...env };
      delete disabled.PXEL_XLSX_SANDBOX_MANIFEST;
      server = await startServer(disabled, base, join(root, 'server.log'));
      const unavailable = await call(input);
      assert.equal(unavailable.status, 503);
      assert.equal((await unavailable.json()).code, 'xlsx_unavailable');
      assert.doesNotMatch(
        readFileSync(join(root, 'server.log'), 'utf8'),
        /hello|Budget|private-marker/,
      );
    } finally {
      await browser?.close();
      await killServer(web);
      await killServer(server);
      if (pgRunning) run(exe('pg_ctl'), ['-D', data, '-m', 'fast', '-w', 'stop'], env);
    }
  },
);
