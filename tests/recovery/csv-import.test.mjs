import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { once } from 'node:events';
import { closeSync, mkdirSync, mkdtempSync, openSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { request as httpRequest } from 'node:http';
import { dirname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';
import { protectLocalPath } from '../../scripts/keyring.mjs';
import { uuid7 } from '../helpers/client.mjs';
import { exe, freePort, killServer, run, startServer } from './private-postgres.mjs';

// Real PostgreSQL and browser fixture, separate from developer documents/ports.
test(
  'CSV publication is atomic, authorized, idempotent and survives PostgreSQL crash; browser flow works',
  { timeout: process.env.PXEL_CSV_VERIFY_WORKSPACE === '1' ? 1800000 : 240000 },
  async () => {
    assert.ok(process.env.PXEL_PG_BIN, 'PXEL_PG_BIN must point to PostgreSQL binaries');
    const local = resolve('.local');
    mkdirSync(local, { recursive: true });
    const root = mkdtempSync(join(local, 'csv-acceptance-'));
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
          active_kek_id: 'csv-test',
          keys: [{ id: 'csv-test', key_hex: key.toString('hex') }],
        }),
        { mode: 0o600 },
      );
    } finally {
      key.fill(0);
    }
    protectLocalPath(keyring);
    const pgPort = await freePort(),
      appPort = await freePort(),
      webPort = await freePort();
    const base = `http://127.0.0.1:${appPort}`,
      web = `http://127.0.0.1:${webPort}`;
    const env = {
      ...process.env,
      DATABASE_URL: `postgresql://pxel:${password}@127.0.0.1:${pgPort}/pxel`,
      PGPASSWORD: password,
      PXEL_DEV_AUTH: '1',
      PXEL_BIND: `127.0.0.1:${appPort}`,
      PXEL_ALLOWED_ORIGINS: `${web},http://127.0.0.1:5173`,
      PXEL_KEYRING_FILE: keyring,
      PXEL_BLOB_ROOT: join(root, 'blobs'),
      PXEL_API_TARGET: base,
      PXEL_WEB_PORT: String(webPort),
      PXEL_WEB_URL: web,
      PXEL_PLAYWRIGHT_AUTH_PATH: join(root, 'browser-auth.json'),
      RUST_TEST_THREADS: '4',
    };
    delete env.PXEL_KEYRING_JSON;
    if (process.platform === 'win32') {
      const aclEnv = { ...env, PXEL_CSV_PRIVATE_ROOT: root, PXEL_CSV_PASSWORD_FILE: passwordFile };
      delete aclEnv.PSModulePath;
      const privateAcl = run(
        'powershell.exe',
        [
          '-NoProfile',
          '-NonInteractive',
          '-Command',
          "$ErrorActionPreference='Stop'; $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User; $rootAcl=Get-Acl -LiteralPath $env:PXEL_CSV_PRIVATE_ROOT; $fileAcl=Get-Acl -LiteralPath $env:PXEL_CSV_PASSWORD_FILE; $rootRules=@($rootAcl.Access); $fileRules=@($fileAcl.Access); $ok=$rootRules.Count -eq 1 -and $fileRules.Count -eq 1 -and $rootRules[0].IdentityReference.Translate([Security.Principal.SecurityIdentifier]) -eq $sid -and $fileRules[0].IdentityReference.Translate([Security.Principal.SecurityIdentifier]) -eq $sid -and $rootRules[0].InheritanceFlags.ToString() -eq 'ContainerInherit, ObjectInherit' -and $fileRules[0].InheritanceFlags.ToString() -eq 'None'; Write-Output $ok",
        ],
        aclEnv,
        true,
      );
      assert.equal(
        privateAcl,
        'True',
        'private directory inherits only the current user; secret file does not inherit',
      );
    }
    const data = join(root, 'data');
    let server,
      vite,
      postgresRunning = false,
      cookie;
    const startPg = () => {
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
      postgresRunning = true;
    };
    const call = (path, body, session = cookie, requestOrigin = web) =>
      fetch(base + path, {
        method: body === undefined ? 'GET' : 'POST',
        headers: {
          Origin: requestOrigin,
          ...(session ? { Cookie: session } : {}),
          'Content-Type': 'application/json',
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(30000),
      });
    const count = async () =>
      (await (await call('/api/documents?schema_version=1')).json()).documents.length;
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
      startPg();
      run(exe('createdb'), ['-h', '127.0.0.1', '-p', String(pgPort), '-U', 'pxel', 'pxel'], env);
      server = await startServer(env, base, join(root, 'server.log'));
      const login = await fetch(base + '/api/dev/session', {
        method: 'POST',
        headers: { Origin: web, 'X-PXEL-DEV-ACCOUNT': 'csv_api_' + randomBytes(8).toString('hex') },
      });
      assert.equal(login.status, 204);
      cookie = login.headers.get('set-cookie').split(';')[0];
      const payload = {
        schema_version: 1,
        title: 'CSV acceptance',
        delimiter: 'comma',
        file_base64: Buffer.from(
          'Code,Amount,Flag\r\n001,12.50,true\r\n=SUM(A1),-3,false\r\n',
        ).toString('base64'),
        column_types: ['text', 'number', 'boolean'],
        header: true,
        document_id: uuid7(),
        request_id: uuid7(),
      };
      const raw = await fetch(base + '/api/imports/csv/preview', {
        method: 'POST',
        headers: { Origin: web, 'Content-Type': 'application/json' },
        body: '{',
      });
      assert.equal(raw.status, 401, 'authorization must precede large-body JSON decoding');
      const slow = httpRequest(new URL('/api/imports/csv/preview', base), {
        method: 'POST',
        headers: {
          Origin: web,
          Cookie: cookie,
          'Content-Type': 'application/json',
          Expect: '100-continue',
        },
      });
      let slowStatus;
      const slowResult = new Promise((resolve, reject) => {
        slow.once('error', reject);
        slow.once('response', (response) => {
          slowStatus = response.statusCode;
          response.resume();
          response.once('end', () => resolve(response.statusCode));
        });
      });
      // Hyper sends Continue when JSON starts reading, after acquiring the
      // shared permit. This avoids racing a probe ahead of admission.
      const admitted = once(slow, 'continue', { signal: AbortSignal.timeout(5000) });
      slow.flushHeaders();
      try {
        await admitted;
        slow.write('{');
        let busy = false;
        const statuses = [];
        for (let i = 0; i < 20; i++) {
          const probe = await call('/api/imports/csv/preview', payload);
          const code = probe.ok ? 'ok' : (await probe.json()).code;
          statuses.push(`${probe.status}:${code}`);
          if (probe.status === 429 && code === 'import_busy') {
            busy = true;
            break;
          }
          await delay(100);
        }
        assert.ok(
          busy,
          'incomplete upload must occupy the shared permit before JSON buffering: ' +
            statuses.join(',') +
            '; slow status=' +
            slowStatus +
            '; headers sent=' +
            slow.headersSent,
        );
        assert.equal(await slowResult, 408, 'unfinished upload must time out');
      } finally {
        slow.destroy();
      }
      assert.equal((await call('/api/imports/csv/preview', payload, '')).status, 401);
      assert.equal(
        (await call('/api/imports/csv/preview', payload, cookie, 'https://foreign.invalid')).status,
        403,
      );
      for (const [changes, status] of [
        [{ schema_version: 2 }, 400],
        [{ unexpected: true }, 422],
        [{ file_base64: Buffer.from('a,"unfinished').toString('base64') }, 422],
        [{ file_base64: Buffer.from([0xff]).toString('base64') }, 422],
        [{ file_base64: Buffer.from('Code,Amount,Flag\n001,NaN,true').toString('base64') }, 422],
      ])
        assert.equal(
          (await call('/api/imports/csv/preview', { ...payload, ...changes })).status,
          status,
        );
      assert.equal(await count(), 0);
      const response = await call('/api/imports/csv/preview', payload);
      assert.equal(response.status, 200);
      const preview = await response.json();
      assert.equal(preview.rows, 3);
      assert.equal(preview.columns, 3);
      assert.deepEqual(preview.sample[2], ['=SUM(A1)', '-3', 'false']);
      assert.equal(await count(), 0, 'preview must not publish a book');
      assert.equal((await call('/api/imports/csv', payload)).status, 400);
      const publish = { ...payload, preview_digest: preview.preview_digest };
      assert.equal(
        (await call('/api/imports/csv', { ...publish, title: 'Changed after preview' })).status,
        409,
      );
      assert.equal(await count(), 0);
      const created = await call('/api/imports/csv', publish);
      assert.equal(created.status, 201);
      const receipt = await created.json();
      const retry = await call('/api/imports/csv', publish);
      assert.equal(retry.status, 201);
      assert.deepEqual(await retry.json(), receipt);
      assert.equal(await count(), 1);
      const changed = { ...payload, title: 'Different book with same identity' };
      const changedPreview = await (await call('/api/imports/csv/preview', changed)).json();
      assert.equal(
        (
          await call('/api/imports/csv', {
            ...changed,
            preview_digest: changedPreview.preview_digest,
          })
        ).status,
        409,
      );
      assert.equal(await count(), 1);
      const before = await (await call('/api/documents/' + payload.document_id)).json();
      assert.equal(before.sheets[0].cells.length, 9);
      assert.ok(
        before.sheets[0].cells.some(
          (cell) => cell.value.type === 'text' && cell.value.value === '=SUM(A1)',
        ),
      );
      await killServer(server);
      server = undefined;
      run(exe('pg_ctl'), ['-D', data, '-m', 'immediate', '-w', 'stop'], env);
      postgresRunning = false;
      startPg();
      server = await startServer(env, base, join(root, 'server.log'));
      const recovered = await (await call('/api/documents/' + payload.document_id)).json();
      assert.equal(recovered.state_hash, before.state_hash);
      assert.deepEqual(recovered.sheets[0].cells, before.sheets[0].cells);
      assert.equal((await call('/api/imports/csv', publish)).status, 201);
      assert.equal(await count(), 1);
      const logout = await fetch(base + '/api/dev/session', {
        method: 'DELETE',
        headers: { Origin: web, Cookie: cookie },
      });
      assert.equal(logout.status, 204);
      assert.equal((await call('/api/imports/csv', publish)).status, 401);
      cookie = undefined;
      const require = createRequire(resolve('apps/web/package.json'));
      const viteFile = join(dirname(require.resolve('vite/package.json')), 'bin/vite.js');
      const log = openSync(join(root, 'vite.log'), 'a');
      vite = spawn(process.execPath, [viteFile, 'apps/web', '--host', '127.0.0.1'], {
        env,
        windowsHide: true,
        stdio: ['ignore', log, log],
      });
      closeSync(log);
      let ready = false;
      for (let i = 0; i < 100; i++) {
        if (vite.exitCode !== null) throw new Error('Vite exited before readiness');
        try {
          if ((await fetch(web, { signal: AbortSignal.timeout(300) })).ok) {
            ready = true;
            break;
          }
        } catch {}
        await delay(100);
      }
      assert.ok(ready, 'Vite readiness timeout');
      const playwright = join(dirname(require.resolve('@playwright/test/package.json')), 'cli.js');
      const browser = spawn(
        process.execPath,
        [playwright, 'test', '--config', 'apps/web/playwright.config.ts', 'csv-import.spec.ts'],
        { env, windowsHide: true, stdio: 'inherit' },
      );
      const [code] = await once(browser, 'exit');
      assert.equal(code, 0, 'CSV browser acceptance failed');
      if (process.env.PXEL_CSV_VERIFY_WORKSPACE === '1') {
        // Windows cannot replace a running server binary during workspace builds.
        await killServer(vite);
        vite = undefined;
        await killServer(server);
        server = undefined;
        const checkLog = openSync(join(root, 'workspace-check.log'), 'a');
        const check = spawn('just', ['check'], {
          env,
          windowsHide: true,
          stdio: ['ignore', checkLog, checkLog],
        });
        closeSync(checkLog);
        const [checkCode] = await once(check, 'exit');
        assert.equal(checkCode, 0, 'workspace check failed; inspect private workspace-check.log');
      }
    } finally {
      if (cookie)
        await fetch(base + '/api/dev/session', {
          method: 'DELETE',
          headers: { Origin: web, Cookie: cookie },
          signal: AbortSignal.timeout(3000),
        }).catch(() => undefined);
      await killServer(vite);
      await killServer(server);
      if (postgresRunning) run(exe('pg_ctl'), ['-D', data, '-m', 'fast', '-w', 'stop'], env);
      // Retain private logs/DB for diagnosis; never recursively remove a computed Windows path.
    }
  },
);
