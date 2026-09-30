import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const env = { ...process.env, PXEL_DEV_AUTH: '1' };
const localDatabase = resolve('.local/database-url');
if (!env.DATABASE_URL && existsSync(localDatabase))
  env.DATABASE_URL = readFileSync(localDatabase, 'utf8').trim();
if (!env.DATABASE_URL) throw new Error('Recovery test needs DATABASE_URL or .local/database-url');

function run(command, args) {
  const result = spawnSync(command, args, { env, stdio: 'inherit', windowsHide: true });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

run('cargo', ['build', '--locked', '-p', 'pxel-server', '--features', 'dev-auth']);
run(process.execPath, [
  '--test',
  '--test-concurrency=1',
  'tests/recovery/server-restart.test.mjs',
  'tests/recovery/convergence-restart.test.mjs',
  'tests/recovery/move-restart.test.mjs',
  'tests/recovery/move-history-restart.test.mjs',
]);
