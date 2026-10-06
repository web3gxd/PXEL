import { spawn, spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { createRequire } from 'node:module';
import { ensureDevelopmentKeyring } from './keyring.mjs';

const webRequire = createRequire(resolve('apps/web/package.json'));
const vite = join(dirname(webRequire.resolve('vite/package.json')), 'bin/vite.js');

const children = [];
let stopping = false;
function stop(code) {
  if (stopping) return;
  stopping = true;
  for (const child of children) {
    if (!child.pid) continue;
    if (process.platform === 'win32')
      spawn('taskkill', ['/pid', String(child.pid), '/t', '/f'], {
        windowsHide: true,
        stdio: 'ignore',
      });
    else {
      try {
        process.kill(-child.pid, 'SIGTERM');
      } catch (error) {
        if (error.code !== 'ESRCH') throw error;
      }
    }
  }
  process.exitCode = code;
}
function start(command, args, env) {
  const child = spawn(command, args, {
    stdio: 'inherit',
    env,
    windowsHide: true,
    detached: process.platform !== 'win32',
  });
  children.push(child);
  child.on('error', (error) => {
    console.error(`Cannot start ${command}: ${error.code}`);
    stop(1);
  });
  child.on('exit', (code) => {
    if (!stopping) stop(code ?? 1);
  });
}
const file = resolve('.local/database-url');
if (!process.env.DATABASE_URL && !existsSync(file))
  throw new Error('Run just infra-up (or just infra-native) first.');
const env = { ...process.env, ...ensureDevelopmentKeyring(), PXEL_DEV_AUTH: '1' };
if (!env.DATABASE_URL) env.DATABASE_URL = readFileSync(file, 'utf8').trim();
const wasm = spawnSync(process.execPath, ['scripts/wasm.mjs'], {
  stdio: 'inherit',
  windowsHide: true,
  env,
});
if (wasm.error || wasm.status !== 0)
  throw new Error('WASM build failed; development stack not started.');
start('cargo', ['run', '-p', 'pxel-server', '--features', 'dev-auth'], env);
start(process.execPath, [vite, 'apps/web', '--host', '127.0.0.1'], env);
process.on('SIGINT', () => stop(0));
process.on('SIGTERM', () => stop(0));
