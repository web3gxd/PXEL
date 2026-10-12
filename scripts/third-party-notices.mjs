// Use the invoking pinned pnpm, not another pnpm found on PATH.
// Keep local installation paths out of the output.
import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const manager = process.env.npm_execpath;
if (!manager) throw new Error('Run pnpm notices:generate or pnpm notices:check.');
const nodeEntry = /\.(?:cjs|mjs|js)$/i.test(manager);
const command = nodeEntry ? process.execPath : manager;
const prefix = nodeEntry ? [manager] : [];
const options = { encoding: 'utf8', windowsHide: true, maxBuffer: 8 * 1024 * 1024 };
const expected = JSON.parse(readFileSync('package.json', 'utf8')).packageManager;
const version = execFileSync(command, [...prefix, '--version'], options).trim();
if (`pnpm@${version}` !== expected) throw new Error(`Use the pinned ${expected}.`);
const groups = JSON.parse(
  execFileSync(command, [...prefix, 'licenses', 'list', '--prod', '--json'], options),
);
const packages = Object.values(groups)
  .flat()
  .sort((a, b) => a.name.localeCompare(b.name, 'en'));
const sections = [
  'PXEL browser runtime — third-party licenses\n\nGenerated from the locked production dependency graph. These components retain\ntheir original licenses; PXEL does not relicense them.\n',
];
for (const pkg of packages) {
  const vendored =
    pkg.name === '@bufbuild/protobuf' && pkg.versions.length === 1 && pkg.versions[0] === '2.15.0';
  const base = vendored ? 'docs/licenses' : pkg.paths[0];
  const files = vendored
    ? ['protobuf-es-2.15.0-APACHE.txt', 'protobuf-es-2.15.0-BSD.txt']
    : readdirSync(base)
        .filter((name) => /^(?:license|licence|copying|notice)(?:[._-].*)?$/i.test(name))
        .sort();
  if (files.length === 0) throw new Error(`Missing license text for ${pkg.name}`);
  sections.push(
    `\n${'='.repeat(72)}\n${pkg.name} ${pkg.versions.join(', ')}\nLicense: ${pkg.license}\n`,
  );
  for (const file of files)
    sections.push(
      `\n--- ${file} ---\n${readFileSync(join(base, file), 'utf8').replaceAll('\r\n', '\n').trim()}\n`,
    );
}
const output = sections.join('');
const path = 'apps/web/public/third-party-notices.txt';
if (process.argv.includes('--check')) {
  if (readFileSync(path, 'utf8').replaceAll('\r\n', '\n') !== output)
    throw new Error('Third-party notices are stale; run pnpm notices:generate.');
  console.log(`Third-party notices: PASS (${packages.length} packages).`);
} else {
  writeFileSync(path, output);
  console.log(`Generated notices for ${packages.length} production packages.`);
}
