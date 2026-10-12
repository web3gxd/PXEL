// Input: pnpm licenses list --prod --json. Keep local installation paths out of the output.
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const groups = JSON.parse(readFileSync(0, 'utf8'));
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
