import { spawnSync } from 'node:child_process';
import { readFileSync, mkdirSync, mkdtempSync, readdirSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join, relative, resolve } from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const buf = require.resolve('@bufbuild/buf/bin/buf');

// Resolve the pinned local plugin for npm scripts and direct Node invocations.
function run(command, args) {
  const result = spawnSync(process.execPath, [buf, ...args], {
    stdio: 'inherit',
    windowsHide: true,
    env: {
      ...process.env,
      PATH: resolve('node_modules/.bin') + delimiter + (process.env.PATH ?? ''),
    },
  });
  if (result.status !== 0) throw new Error(`${command} failed (${result.status})`);
}
const check = process.argv.includes('--check');
const temporary = check ? mkdtempSync(join(tmpdir(), 'pxel-proto-')) : undefined;
try {
  run('buf', ['lint']);
  const descriptor = check ? join(temporary, 'wire-descriptor.bin') : 'proto/wire-descriptor.bin';
  run('buf', ['build', '--as-file-descriptor-set', '-o', descriptor]);
  if (check) {
    run('buf', ['generate', '--path', 'proto/pxel/wire', '--output', temporary]);
    const files = ['proto/wire-descriptor.bin'];
    function walk(dir) {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) walk(path);
        else files.push(relative(temporary, path));
      }
    }
    walk(join(temporary, 'apps/web/src/generated'));
    for (const path of files) {
      const generated = path === 'proto/wire-descriptor.bin' ? descriptor : join(temporary, path);
      if (!existsSync(path) || !readFileSync(path).equals(readFileSync(generated))) {
        throw new Error(`Generated protocol drift: ${path}. Run pnpm proto:generate.`);
      }
    }
  } else {
    mkdirSync(resolve('apps/web/src/generated'), { recursive: true });
    run('buf', ['generate', '--path', 'proto/pxel/wire']);
  }
} finally {
  if (
    temporary &&
    resolve(temporary).startsWith(
      resolve(tmpdir()) + (process.platform === 'win32' ? '\\' : '/') + 'pxel-proto-',
    )
  ) {
    rmSync(temporary, { recursive: true, force: true });
  }
}
