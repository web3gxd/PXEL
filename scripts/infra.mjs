import { spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve, join } from 'node:path';

const local = resolve('.local');
mkdirSync(local, { recursive: true });
const passwordFile = join(local, 'db-password');
if (!existsSync(passwordFile))
  writeFileSync(passwordFile, randomBytes(24).toString('hex'), { mode: 0o600 });
const password = readFileSync(passwordFile, 'utf8').trim();
const native = process.argv.includes('--native');
const port = process.env.PXEL_DB_PORT ?? '55432';
if (!/^\d{4,5}$/.test(port)) throw new Error('Invalid PXEL_DB_PORT');
const databaseUrl = `postgresql://pxel:${encodeURIComponent(password)}@127.0.0.1:${port}/pxel`;
writeFileSync(join(local, 'database-url'), databaseUrl, { mode: 0o600 });
function run(command, args, options = {}) {
  const result = spawnSync(command, args, { stdio: 'inherit', windowsHide: true, ...options });
  if (result.error) throw new Error(`Cannot start ${command}: ${result.error.code}`);
  if (result.status !== 0) throw new Error(`${command} failed (${result.status})`);
  return result;
}
if (native) {
  const bin = process.env.PXEL_PG_BIN ?? '';
  const exe = (name) =>
    bin ? join(bin, name + (process.platform === 'win32' ? '.exe' : '')) : name;
  const data = join(local, 'postgres');
  if (!existsSync(join(data, 'PG_VERSION'))) {
    run(exe('initdb'), [
      '-D',
      data,
      '-U',
      'pxel',
      '--encoding=UTF8',
      '--locale=C',
      '--auth-host=scram-sha-256',
      '--auth-local=scram-sha-256',
      `--pwfile=${passwordFile}`,
    ]);
  }
  const status = spawnSync(exe('pg_ctl'), ['-D', data, 'status'], {
    windowsHide: true,
    stdio: 'ignore',
  });
  if (status.status !== 0) {
    run(exe('pg_ctl'), [
      '-D',
      data,
      '-l',
      join(local, 'postgres.log'),
      '-o',
      `-h 127.0.0.1 -p ${port} -c synchronous_commit=on -c fsync=on -c full_page_writes=on`,
      '-w',
      'start',
    ]);
  }
  const env = { ...process.env, PGPASSWORD: password };
  const existing = spawnSync(
    exe('psql'),
    [
      '-h',
      '127.0.0.1',
      '-p',
      port,
      '-U',
      'pxel',
      '-d',
      'postgres',
      '-tAc',
      "SELECT 1 FROM pg_database WHERE datname='pxel'",
    ],
    { env, encoding: 'utf8', windowsHide: true },
  );
  if (existing.status !== 0) throw new Error('Private PostgreSQL authentication failed');
  if (existing.stdout.trim() !== '1')
    run(exe('createdb'), ['-h', '127.0.0.1', '-p', port, '-U', 'pxel', 'pxel'], { env });
} else {
  run('docker', ['compose', 'up', '-d', '--wait', 'postgres']);
}
if (!process.argv.includes('--database-only')) {
  run('cargo', ['run', '-p', 'pxel-server', '--', 'migrate'], {
    env: { ...process.env, DATABASE_URL: databaseUrl },
  });
}
console.log(
  `PXEL private PostgreSQL ready on loopback:${port}. Credentials saved in ignored .local files.`,
);
