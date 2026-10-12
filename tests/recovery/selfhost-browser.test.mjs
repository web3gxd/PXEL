import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { protectLocalPath } from '../../scripts/keyring.mjs';
import { exe, freePort, killServer, run, startServer } from './private-postgres.mjs';

const webRequire = createRequire(resolve('apps/web/package.json'));
const { chromium, expect } = webRequire('@playwright/test');

test(
  'self-hosted first run, master administration and forced password change in Chromium',
  { timeout: 150000 },
  async () => {
    if (!process.env.PXEL_PG_BIN)
      throw new Error('Set PXEL_PG_BIN to private PostgreSQL binaries.');
    mkdirSync('.local', { recursive: true });
    const root = mkdtempSync(resolve('.local/selfhost-browser-'));
    protectLocalPath(root, true);
    const pgPort = await freePort(),
      appPort = await freePort();
    const base = `http://localhost:${appPort}`,
      password = randomBytes(24).toString('hex');
    const token = randomBytes(32).toString('hex'),
      masterPassword = randomBytes(24).toString('hex');
    const temporaryPassword = randomBytes(24).toString('hex'),
      personalPassword = randomBytes(24).toString('hex');
    const pwfile = join(root, 'pg-password'),
      tokenfile = join(root, 'bootstrap-token'),
      keyfile = join(root, 'keyring.json');
    writeFileSync(pwfile, password, { mode: 0o600 });
    protectLocalPath(pwfile);
    writeFileSync(tokenfile, token, { mode: 0o600 });
    protectLocalPath(tokenfile);
    writeFileSync(
      keyfile,
      JSON.stringify({
        schema_version: 1,
        active_kek_id: 'browser-test',
        keys: [{ id: 'browser-test', key_hex: randomBytes(32).toString('hex') }],
      }),
      { mode: 0o600 },
    );
    protectLocalPath(keyfile);
    const env = {
      ...process.env,
      DATABASE_URL: `postgresql://pxel:${password}@127.0.0.1:${pgPort}/pxel`,
      PGPASSWORD: password,
      PXEL_DEV_AUTH: '0',
      PXEL_BIND: `127.0.0.1:${appPort}`,
      PXEL_ALLOWED_ORIGINS: base,
      PXEL_BLOB_ROOT: join(root, 'blobs'),
      PXEL_BOOTSTRAP_TOKEN_FILE: tokenfile,
      PXEL_KEYRING_FILE: keyfile,
    };
    delete env.PXEL_KEYRING_JSON;
    const data = join(root, 'data');
    let server,
      browser,
      postgres = false;
    const errors = [];
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
          `--pwfile=${pwfile}`,
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
          `-h 127.0.0.1 -p ${pgPort} -c fsync=on -c synchronous_commit=on -c full_page_writes=on`,
          '-w',
          'start',
        ],
        env,
      );
      postgres = true;
      run(exe('createdb'), ['-h', '127.0.0.1', '-p', String(pgPort), '-U', 'pxel', 'pxel'], env);
      server = await startServer(env, base, join(root, 'server.log'));
      browser = await chromium.launch();
      const master = await browser.newContext({
        viewport: { width: 1440, height: 1000 },
        deviceScaleFactor: 2,
      });
      const page = await master.newPage();
      page.on('pageerror', () => errors.push('master page error'));
      await page.goto(base);
      await expect(page.getByRole('heading', { name: 'Начнём с главного' })).toBeVisible();
      await page.screenshot({ path: join(root, 'welcome-desktop.png'), fullPage: true });
      await page.setViewportSize({ width: 390, height: 844 });
      assert.equal(
        await page.evaluate(
          () =>
            document.documentElement.scrollWidth <= innerWidth &&
            [...document.querySelectorAll('.auth-story, .auth-card, .auth-story h1')].every(
              (el) =>
                el.getBoundingClientRect().right <= innerWidth && el.scrollWidth <= el.clientWidth,
            ),
        ),
        true,
      );
      await page.screenshot({ path: join(root, 'welcome-mobile.png'), fullPage: true });
      await page.getByRole('button', { name: 'Настроить пространство' }).click();
      await page.screenshot({ path: join(root, 'setup-mobile.png'), fullPage: true });
      await page.getByLabel('Код установки').fill(token);
      await page.getByLabel(/^Логин/).fill('master');
      await page.getByLabel(/^Пароль/).fill(masterPassword);
      await page.getByLabel('Повторите пароль').fill(masterPassword);
      await page.getByRole('button', { name: 'Создать мастер-аккаунт', exact: true }).click();
      await expect(page.getByRole('heading', { name: 'Рабочие книги' })).toBeVisible({
        timeout: 15000,
      });
      assert.equal(
        await page.evaluate(
          () =>
            document.documentElement.scrollWidth <= innerWidth &&
            [...document.querySelectorAll('.auth-story, .auth-card, .auth-story h1')].every(
              (el) =>
                el.getBoundingClientRect().right <= innerWidth && el.scrollWidth <= el.clientWidth,
            ),
        ),
        true,
      );
      await page.setViewportSize({ width: 1440, height: 1000 });
      const cookies = await master.cookies(`${base}/api/auth/status`);
      assert.equal(
        cookies.some((c) => c.httpOnly && c.secure && c.sameSite === 'Strict' && c.path === '/api'),
        true,
      );
      await page.getByRole('button', { name: 'Пользователи', exact: true }).click();
      const admin = page.getByRole('dialog');
      await expect(admin.getByText('Мастер-аккаунт', { exact: true })).toBeVisible();
      await admin.getByLabel(/^Логин/).fill('colleague');
      await admin.getByLabel(/^Временный пароль/).fill(temporaryPassword);
      await admin.getByRole('button', { name: 'Создать пользователя', exact: true }).click();
      await expect(admin.getByText('Ожидает смены пароля', { exact: true })).toBeVisible({
        timeout: 15000,
      });
      await page.screenshot({ path: join(root, 'accounts-desktop.png'), fullPage: true });
      await admin.getByRole('button', { name: 'Закрыть окно', exact: true }).click();
      const member = await browser.newContext({ viewport: { width: 390, height: 844 } });
      const user = await member.newPage();
      user.on('pageerror', () => errors.push('member page error'));
      await user.goto(base);
      await expect(user.getByRole('heading', { name: 'Войти в PXEL' })).toBeVisible();
      await user.getByLabel(/^Логин/).fill('colleague');
      await user.getByLabel(/^Пароль/).fill(temporaryPassword);
      await user.getByRole('button', { name: 'Войти', exact: true }).click();
      await expect(user.getByRole('heading', { name: 'Новый личный пароль' })).toBeVisible({
        timeout: 15000,
      });
      await user.screenshot({ path: join(root, 'password-change-mobile.png'), fullPage: true });
      await user.getByLabel('Текущий пароль', { exact: true }).fill(temporaryPassword);
      await user.getByLabel(/^Новый пароль/).fill(personalPassword);
      await user.getByLabel('Повторите пароль').fill(personalPassword);
      await user.getByRole('button', { name: 'Сохранить пароль', exact: true }).click();
      await expect(user.getByRole('heading', { name: 'Рабочие книги' })).toBeVisible({
        timeout: 15000,
      });
      await expect(user.getByRole('button', { name: 'Пользователи', exact: true })).toHaveCount(0);
      await user.getByRole('button', { name: 'Создать таблицу', exact: true }).click();
      await expect(user.getByRole('button', { name: 'О PXEL и лицензии' })).toBeVisible({
        timeout: 15000,
      });
      const documentId = new URL(user.url()).searchParams.get('document');
      assert.ok(documentId);
      await user.getByRole('button', { name: 'О PXEL и лицензии' }).click();
      await expect(
        user.getByRole('dialog').getByRole('link', { name: 'Исходный код' }),
      ).toBeVisible();
      await user.getByRole('dialog').getByRole('button', { name: 'Закрыть окно' }).click();
      await page.reload();
      await expect(page.locator(`[data-book-id="${documentId}"]`)).toBeVisible({ timeout: 15000 });
      await page.locator(`[data-book-id="${documentId}"] .book-title`).click();
      await expect(page.getByRole('button', { name: 'О PXEL и лицензии' })).toBeVisible();
      // The master disables the member; the member's revoked socket/session refreshes AuthGate.
      await page.goto(base);
      await page.getByRole('button', { name: 'Пользователи', exact: true }).click();
      page.once('dialog', (dialog) => dialog.accept());
      await page
        .getByRole('dialog')
        .getByRole('button', { name: 'Отключить', exact: true })
        .click();
      await expect(page.getByRole('dialog').getByText('Отключён', { exact: true })).toBeVisible();
      await expect(user.getByRole('heading', { name: 'Войти в PXEL' })).toBeVisible({
        timeout: 20000,
      });
      assert.equal(await user.evaluate(() => localStorage.length), 0);
      assert.deepEqual(errors, []);
      console.log(`Self-host browser screenshots retained privately: ${root}`);
    } finally {
      await browser?.close();
      await killServer(server);
      if (postgres) run(exe('pg_ctl'), ['-D', data, '-m', 'fast', '-w', 'stop'], env);
    }
  },
);
