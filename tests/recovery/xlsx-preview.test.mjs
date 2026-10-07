import assert from 'node:assert/strict';
import { randomBytes, createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync, copyFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { dirname } from 'node:path';
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import { request as httpRequest } from 'node:http';
import { test } from 'node:test';
import { uuid7 } from '../helpers/client.mjs';
import { scalarXlsx } from '../helpers/xlsx-profile.mjs';
import { protectLocalPath } from '../../scripts/keyring.mjs';
import { exe, freePort, killServer, run, startServer } from './private-postgres.mjs';

test(
  'real XLSX preview validates candidates and publication is atomic, retry-stable and recoverable',
  { timeout: process.env.PXEL_XLSX_LARGE === '1' ? 600000 : 180000 },
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
        signal: AbortSignal.timeout(60000),
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
      const oversizedSource = await call({
        ...input,
        file_base64: Buffer.alloc(32 * 1024 * 1024 + 1).toString('base64'),
      }).catch((cause) => {
        throw new Error('Decoded source boundary transport failed', { cause });
      });
      assert.equal(oversizedSource.status, 413, 'decoded source above 32MiB rejected');
      assert.equal((await oversizedSource.json()).code, 'xlsx_limit');
      const oversizedBodyStatus = await new Promise((resolve, reject) => {
        const request = httpRequest(base + '/api/imports/xlsx/preview', {
          method: 'POST',
          agent: false,
          headers: {
            Origin: origin,
            Cookie: cookie,
            'Content-Type': 'application/json',
            'Content-Length': String(48 * 1024 * 1024 + 1),
            Expect: '100-continue',
          },
        });
        request.once('response', (response) => {
          const status = response.statusCode;
          response.resume();
          response.once('end', () => {
            resolve(status);
            request.destroy();
          });
        });
        request.once('continue', () => request.destroy(new Error('Oversized body was admitted')));
        request.once('error', reject);
        request.setTimeout(30000, () =>
          request.destroy(new Error('Oversized body rejection timed out')),
        );
        request.flushHeaders();
      });
      assert.equal(oversizedBodyStatus, 413, 'body above 48MiB refused before upload');
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
      const publicationPreviewResponse = await call({ ...input, title: 'Импорт XLSX' });
      assert.equal(publicationPreviewResponse.status, 200);
      const publicationPreview = await publicationPreviewResponse.json();
      const publication = {
        ...input,
        selected_sheets: [0, 1],
        title: 'Импорт XLSX',
        document_id: uuid7(),
        request_id: uuid7(),
        preview_digest: publicationPreview.preview_digest,
        accept_reported: false,
      };
      const publish = (payload, session = cookie, requestOrigin = origin) =>
        fetch(base + '/api/imports/xlsx', {
          method: 'POST',
          headers: {
            Origin: requestOrigin,
            'Content-Type': 'application/json',
            ...(session ? { Cookie: session } : {}),
          },
          body: JSON.stringify(payload),
          signal: AbortSignal.timeout(60000),
        });
      assert.equal(
        (await publish(null, '')).status,
        401,
        'publication authorizes before DTO parsing',
      );
      assert.equal(
        (await publish(publication, cookie, 'https://foreign.example')).status,
        403,
        'publication rejects a foreign origin',
      );
      assert.equal((await publish({ ...publication, preview_digest: '0'.repeat(64) })).status, 409);
      assert.equal((await publish({ ...publication, title: 'Изменено' })).status, 409);
      assert.equal((await publish({ ...publication, schema_version: 2 })).status, 400);
      assert.equal((await publish({ ...publication, document_id: 'invalid' })).status, 400);
      const published = await publish(publication);
      assert.equal(published.status, 201);
      const receipt = await published.json();
      assert.equal(receipt.document_id, publication.document_id);
      const retry = await publish(publication);
      assert.equal(retry.status, 201);
      assert.deepEqual(await retry.json(), receipt);
      const before = await (
        await fetch(base + '/api/documents/' + publication.document_id, {
          headers: { Cookie: cookie },
        })
      ).json();
      assert.equal(before.sheets.length, 2);
      assert.equal(
        before.sheets.reduce((sum, sheet) => sum + sheet.cells.length, 0),
        5,
      );
      const unsupportedInput = {
        ...input,
        title: 'Неподдерживаемая функция',
        file_base64: readFileSync('fixtures/xlsx/unknown-function.xlsx').toString('base64'),
      };
      const unsupportedPreviewResponse = await call(unsupportedInput);
      assert.equal(unsupportedPreviewResponse.status, 200);
      const unsupportedPreview = await unsupportedPreviewResponse.json();
      assert.deepEqual(unsupportedPreview.formula_issues, [
        { sheet_index: 0, function: 'PXELUNKNOWN', cells: 1 },
      ]);
      const unsupportedPublication = {
        ...publication,
        ...unsupportedInput,
        document_id: uuid7(),
        request_id: uuid7(),
        preview_digest: unsupportedPreview.preview_digest,
      };
      assert.equal((await publish(unsupportedPublication)).status, 409);
      assert.equal(
        (await publish({ ...unsupportedPublication, accept_reported: true })).status,
        201,
      );
      const unsupportedHead = await (
        await fetch(base + '/api/documents/' + unsupportedPublication.document_id, {
          headers: { Cookie: cookie },
        })
      ).json();
      assert.equal(
        unsupportedHead.sheets[0].cells.find((cell) => cell.value.type === 'formula').value.value
          .source,
        '=PXELUNKNOWN(A1)',
      );
      const missingSheet = await call({
        ...input,
        file_base64: readFileSync('fixtures/xlsx/cross_sheet_formula.xlsx').toString('base64'),
        selected_sheets: [0],
      });
      assert.equal(missingSheet.status, 422);
      const conformanceBooks = [];
      for (const name of ['styled_profile', 'excel-shared-formulas', 'single_sheet_merge']) {
        const source = {
          ...input,
          title: 'Conformance ' + name,
          file_base64: readFileSync('fixtures/xlsx/' + name + '.xlsx').toString('base64'),
        };
        const response = await call(source);
        assert.equal(response.status, 200, name + ' preview');
        const preview = await response.json();
        const payload = {
          ...publication,
          ...source,
          selected_sheets: preview.selected_sheets,
          document_id: uuid7(),
          request_id: uuid7(),
          preview_digest: preview.preview_digest,
          accept_reported: true,
        };
        assert.equal((await publish(payload)).status, 201, name + ' publication');
        const head = await (
          await fetch(base + '/api/documents/' + payload.document_id, {
            headers: { Cookie: cookie },
          })
        ).json();
        const sheet = head.sheets[0];
        const cell = (row, column) =>
          sheet.cells.find(
            (cell) => cell.row_id === sheet.rows[row] && cell.column_id === sheet.columns[column],
          );
        if (name === 'styled_profile') {
          assert.equal(sheet.styles.length, 3);
          assert.equal(cell(0, 1), undefined, 'styled empty cell has no authored value');
          const expected = {
            font_family: 'Courier New',
            font_size_hundredth_points: 1200,
            font_bold: true,
            font_italic: true,
            font_underline: 'double',
            font_strike: true,
            font_color: [255, 128, 0],
            fill: [12, 34, 56],
            horizontal: 'center',
            vertical: 'center',
            text_overflow: 'wrap',
            number_format: 'decimal2',
          };
          for (const { style } of sheet.styles) {
            for (const [key, value] of Object.entries(expected))
              assert.deepEqual(style[key], value, key);
            for (const side of ['top', 'right', 'bottom', 'left'])
              assert.deepEqual(style['border_' + side], { line: 'double', color: [64, 96, 128] });
          }
          assert.equal(cell(0, 2).value.value.source, '=1+2');
        } else if (name === 'excel-shared-formulas') {
          for (let row = 1; row < 6; row++) {
            const n = row + 1;
            assert.equal(
              cell(row, 4).value.value.source,
              row === 1 ? `=A${n}*2+B$1+$C${n}+$D$1` : `=((((A${n}*2)+B$1)+$C${n})+$D$1)`,
            );
            assert.equal(cell(row, 5).value.value.source, `=SUM(A${n}:B${n})`);
            assert.equal(
              cell(row, 6).value.value.source,
              row === 1 ? `=IF(A${n}>5,"A1 text",$B${n})` : `=IF((A${n}>5),"A1 text",$B${n})`,
            );
          }
        } else {
          assert.deepEqual(sheet.merges, [
            {
              first_row_id: sheet.rows[0],
              last_row_id: sheet.rows[0],
              first_column_id: sheet.columns[4],
              last_column_id: sheet.columns[5],
            },
          ]);
        }
        conformanceBooks.push({
          document: payload.document_id,
          hash: head.state_hash,
          version: head.version,
        });
      }
      if (process.env.PXEL_XLSX_LARGE === '1') {
        const source = {
          ...input,
          title: '100000-cell profile',
          file_base64: scalarXlsx(100000).toString('base64'),
        };
        const started = performance.now();
        const response = await call(source);
        assert.equal(response.status, 200, '100000-cell preview');
        const preview = await response.json();
        assert.equal(preview.sheets[0].cells, 100000);
        const payload = {
          ...publication,
          ...source,
          document_id: uuid7(),
          request_id: uuid7(),
          selected_sheets: [0],
          preview_digest: preview.preview_digest,
          accept_reported: false,
        };
        const imported = await publish(payload);
        assert.equal(imported.status, 201, '100000-cell publication');
        const head = await (
          await fetch(base + '/api/documents/' + payload.document_id, {
            headers: { Cookie: cookie },
          })
        ).json();
        assert.equal(head.sheets[0].cells.length, 100000);
        const sheet = head.sheets[0];
        assert.equal(
          sheet.cells.find(
            (cell) => cell.row_id === sheet.rows[199] && cell.column_id === sheet.columns[499],
          ).value.value,
          100000,
        );
        conformanceBooks.push({
          document: payload.document_id,
          hash: head.state_hash,
          version: head.version,
        });
        assert.equal(
          (await call({ ...source, file_base64: scalarXlsx(100001).toString('base64') })).status,
          422,
          '100001-cell package refused',
        );
        console.log(
          'Large XLSX profile preview/publication verified in ' +
            Math.round(performance.now() - started) +
            'ms',
        );
      }
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
        await dialog
          .getByLabel('XLSX-файл')
          .setInputFiles(resolve('fixtures/xlsx/unknown-function.xlsx'));
        await dialog.getByRole('button', { name: 'Предпросмотр', exact: true }).click();
        await dialog.getByText('функция PXELUNKNOWN не поддерживается', { exact: false }).waitFor();
        assert.equal(
          await dialog.getByRole('button', { name: 'Создать книгу', exact: true }).isDisabled(),
          true,
        );
        await dialog.getByLabel('Принимаю перечисленные ограничения совместимости').check();
        assert.equal(
          await dialog.getByRole('button', { name: 'Создать книгу', exact: true }).isEnabled(),
          true,
        );
        await dialog
          .getByLabel('XLSX-файл')
          .setInputFiles(resolve('fixtures/xlsx/minimal_scalars.xlsx'));
        await dialog.getByRole('button', { name: 'Предпросмотр', exact: true }).click();
        await dialog.getByRole('status').filter({ hasText: 'Проверено листов: 2.' }).waitFor();
        let lost;
        if (width === 390) {
          await page.route('**/api/imports/xlsx', async (route) => {
            if (lost) {
              await route.continue();
              return;
            }
            lost = route.request().postData();
            const response = await route.fetch();
            assert.equal(response.status(), 201);
            await route.abort('failed');
          });
        }
        await dialog.getByRole('button', { name: 'Создать книгу', exact: true }).click();
        if (width === 390) {
          await dialog.getByRole('alert').waitFor();
          await dialog.getByRole('button', { name: 'Создать книгу', exact: true }).click();
        }
        await page.getByTestId('connection-status').filter({ hasText: 'Подключено' }).waitFor();
        const browserId = new URL(page.url()).searchParams.get('document');
        assert.ok(browserId);
        if (lost) assert.equal(JSON.parse(lost).document_id, browserId);
        const head = await (await page.request.get(origin + '/api/documents/' + browserId)).json();
        assert.equal(head.sheets.length, 2);
        assert.equal(
          head.sheets[0].cells.filter((cell) => cell.value.type === 'formula').length,
          1,
        );
        await page.reload();
        await page.getByRole('textbox', { name: 'Значение ячейки', exact: true }).waitFor();
        const recovered = await (
          await page.request.get(origin + '/api/documents/' + browserId)
        ).json();
        assert.equal(recovered.state_hash, head.state_hash);
        assert.deepEqual(errors, []);
        await context.close();
      }
      if (process.env.PXEL_XLSX_LARGE === '1') {
        const context = await browser.newContext({
          viewport: { width: 1440, height: 900 },
          deviceScaleFactor: 2,
        });
        const separator = cookie.indexOf('=');
        await context.addCookies([
          {
            name: cookie.slice(0, separator),
            value: cookie.slice(separator + 1),
            domain: new URL(origin).hostname,
            path: '/api',
            httpOnly: true,
            sameSite: 'Strict',
          },
        ]);
        const page = await context.newPage();
        const errors = [];
        page.on('pageerror', (error) => errors.push(error.message));
        const large = conformanceBooks.at(-1);
        assert.equal(
          (await context.request.get(origin + '/api/documents/' + large.document)).status(),
          200,
          'large browser context has the owning member session',
        );
        await page.goto(origin + '/?document=' + large.document);
        await page
          .getByTestId('connection-status')
          .filter({ hasText: 'Подключено' })
          .waitFor({ timeout: 60000 });
        const address = page.getByRole('textbox', { name: 'Ячейка или диапазон', exact: true });
        await address.fill('SF200');
        await address.press('Enter');
        await page.waitForFunction(
          () => document.querySelector('[aria-label="Значение ячейки"]')?.value === '100000',
          undefined,
          { timeout: 60000 },
        );
        await page.reload();
        await page
          .getByTestId('connection-status')
          .filter({ hasText: 'Подключено' })
          .waitFor({ timeout: 60000 });
        await address.fill('SF200');
        await address.press('Enter');
        await page.waitForFunction(
          () => document.querySelector('[aria-label="Значение ячейки"]')?.value === '100000',
          undefined,
          { timeout: 60000 },
        );
        assert.deepEqual(errors, []);
        await context.close();
      }
      await browser.close();
      browser = undefined;
      await killServer(web);
      web = undefined;
      await killServer(server);
      server = undefined;
      // Crash only this fixture's PostgreSQL after acknowledged publication.
      run(exe('pg_ctl'), ['-D', data, '-m', 'immediate', '-w', 'stop'], env);
      pgRunning = false;
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
      server = await startServer(env, base, join(root, 'server.log'));
      const durableRetry = await publish(publication);
      assert.equal(durableRetry.status, 201);
      assert.deepEqual(await durableRetry.json(), receipt);
      const crashRecovered = await (
        await fetch(base + '/api/documents/' + publication.document_id, {
          headers: { Cookie: cookie },
        })
      ).json();
      assert.equal(crashRecovered.state_hash, before.state_hash);
      assert.equal(crashRecovered.version, before.version);
      for (const book of conformanceBooks) {
        const restored = await (
          await fetch(base + '/api/documents/' + book.document, {
            headers: { Cookie: cookie },
          })
        ).json();
        assert.equal(restored.state_hash, book.hash);
        assert.equal(restored.version, book.version);
      }
      await killServer(server);
      server = undefined;
      const disabled = { ...env };
      delete disabled.PXEL_XLSX_SANDBOX_MANIFEST;
      server = await startServer(disabled, base, join(root, 'server.log'));
      const unavailable = await call(input);
      assert.equal(unavailable.status, 503);
      assert.equal((await unavailable.json()).code, 'xlsx_unavailable');
      const recovered = await (
        await fetch(base + '/api/documents/' + publication.document_id, {
          headers: { Cookie: cookie },
        })
      ).json();
      assert.equal(recovered.state_hash, before.state_hash);
      assert.equal(recovered.version, before.version);
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
