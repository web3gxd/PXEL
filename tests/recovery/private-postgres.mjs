import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { openSync, closeSync } from 'node:fs';
import { createServer } from 'node:net';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

export const exe = (name) =>
  join(process.env.PXEL_PG_BIN ?? '', name + (process.platform === 'win32' ? '.exe' : ''));

export function run(command, args, env = process.env, capture = false, timeoutMs = 30000) {
  // pg_ctl's server process can inherit captured pipe handles, leaving spawnSync
  // waiting for EOF forever even after pg_ctl itself exits.
  const result = spawnSync(command, args, {
    env,
    encoding: 'utf8',
    windowsHide: true,
    stdio: capture ? ['ignore', 'pipe', 'pipe'] : 'ignore',
    timeout: timeoutMs,
  });
  if (result.error || result.status !== 0)
    throw new Error(
      `${command} failed: ${result.error?.code ?? result.stderr?.trim() ?? result.status}`,
    );
  return capture ? result.stdout.trim() : '';
}

export async function freePort() {
  const server = createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const port = server.address().port;
  server.close();
  await once(server, 'close');
  return port;
}

export async function startServer(env, base, logFile) {
  const binary = env.PXEL_TEST_SERVER_BIN
    ? resolve(env.PXEL_TEST_SERVER_BIN)
    : resolve('target/debug/pxel-server' + (process.platform === 'win32' ? '.exe' : ''));
  const log = openSync(logFile, 'a');
  const child = spawn(binary, [], { env, stdio: ['ignore', log, log], windowsHide: true });
  closeSync(log);
  let startError;
  child.on('error', (error) => {
    startError = error;
  });
  for (let attempt = 0; attempt < 150; attempt++) {
    if (startError || child.exitCode !== null || child.signalCode !== null)
      throw startError ?? new Error('server exited before ready; inspect private server.log');
    try {
      if ((await fetch(base + '/readyz', { signal: AbortSignal.timeout(300) })).ok) return child;
    } catch {}
    await delay(100);
  }
  child.kill('SIGKILL');
  throw new Error('server readiness timeout');
}

export async function killServer(child) {
  if (child && child.exitCode === null && child.signalCode === null) {
    const exit = once(child, 'exit');
    child.kill('SIGKILL');
    await exit;
  }
}
