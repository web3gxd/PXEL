import { execFileSync } from 'node:child_process';
import { existsSync, lstatSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function privatePath(path) {
  const parts = path.replaceAll('\\', '/').toLowerCase().split('/');
  const name = parts.at(-1);
  return (
    parts.some((part) =>
      [
        '.local',
        '.tools',
        '.git',
        '.venv',
        '__pycache__',
        '.unsnooze',
        'node_modules',
        'target',
        'dist',
        'coverage',
        'test-results',
        'playwright-report',
      ].includes(part),
    ) ||
    (name.startsWith('.env') && name !== '.env.example') ||
    /\.(?:log|tmp|bak|key|pem|p12|pfx|dump|backup|exe|dll|sqlite3?)$/.test(name)
  );
}

function check() {
  const paths = execFileSync(
    'git',
    ['ls-files', '--cached', '--others', '--exclude-standard', '-z'],
    { encoding: 'utf8', windowsHide: true },
  )
    .split('\0')
    .filter(Boolean);
  const problems = [];
  for (const path of new Set(paths)) {
    if (!existsSync(path)) continue; // staged deletions
    if (privatePath(path)) problems.push(`Private/generated path: ${path}`);
    const stat = lstatSync(path);
    if (!stat.isFile()) problems.push(`Non-regular published file: ${path}`);
    if (stat.size > 10 * 1024 * 1024) problems.push(`Published file exceeds 10 MiB: ${path}`);
  }
  for (const path of ['LICENSE', 'NOTICE', 'CITATION.cff', 'SECURITY.md', 'CONTRIBUTING.md'])
    if (!existsSync(path)) problems.push(`Missing publication document: ${path}`);
  for (const path of ['package.json', 'apps/web/package.json']) {
    if (JSON.parse(readFileSync(path, 'utf8')).license !== 'AGPL-3.0-only')
      problems.push(`Missing AGPL license metadata: ${path}`);
  }
  if (
    !existsSync('apps/web/public/license.txt') ||
    readFileSync('LICENSE', 'utf8').replaceAll('\r\n', '\n') !==
      readFileSync('apps/web/public/license.txt', 'utf8').replaceAll('\r\n', '\n')
  )
    problems.push('Browser AGPL text must match LICENSE.');
  if (problems.length) {
    for (const issue of problems) console.error(issue);
    process.exitCode = 1;
  } else console.log(`Repository publication paths and metadata: PASS (${paths.length} paths).`);
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) check();
