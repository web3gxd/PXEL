import { randomBytes } from 'node:crypto';
import { protectLocalPath } from './keyring.mjs';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

const path = resolve(process.argv[2] ?? '.local/bootstrap-token');
try {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  writeFileSync(path, randomBytes(32).toString('hex') + '\n', { flag: 'wx', mode: 0o600 });
  protectLocalPath(path);
  console.log(`Created private bootstrap-token file: ${path}`);
  console.log(
    'Open it privately on the server and enter the code in first-run setup. The code is never printed here.',
  );
} catch (error) {
  console.error(
    error.code === 'EEXIST'
      ? 'Bootstrap-token file already exists; it was not overwritten.'
      : 'Could not create the private bootstrap-token file. Check the destination and permissions.',
  );
  process.exitCode = 1;
}
