import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { dirname, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const cargoTargetDir = resolve(root, process.env.CARGO_TARGET_DIR ?? 'target');
const version = '0.2.129';
const local = resolve(
  root,
  `.tools/wasm-bindgen/wasm-bindgen-${version}-x86_64-pc-windows-msvc/wasm-bindgen.exe`,
);
const cli =
  process.env.PXEL_WASM_BINDGEN ??
  (process.platform === 'win32' && existsSync(local) ? local : 'wasm-bindgen');
const output = resolve(root, 'apps/web/src/generated/wasm');
const manifest = readFileSync(resolve(root, 'crates/spreadsheet-wasm/Cargo.toml'), 'utf8');
if (!manifest.includes(`wasm-bindgen = "=${version}"`))
  throw new Error('WASM CLI and crate version pin differ');
function run(command, args, capture = false) {
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: capture ? 'pipe' : 'inherit',
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.error || result.status !== 0)
    throw new Error(`WASM build command failed: ${command} (${result.status ?? 'unavailable'})`);
  return result.stdout?.trim();
}
if (run(cli, ['--version'], true) !== `wasm-bindgen ${version}`)
  throw new Error(`Expected wasm-bindgen ${version}`);
// Remove only this generated-output directory's files, checked under the workspace.
if (!output.startsWith(root + sep) || !output.endsWith(`${sep}generated${sep}wasm`))
  throw new Error('Invalid WASM output path');
mkdirSync(output, { recursive: true });
for (const file of readdirSync(output, { withFileTypes: true })) {
  if (!file.isFile()) throw new Error('Unexpected directory inside WASM generated output');
  rmSync(resolve(output, file.name));
}
run('cargo', [
  'build',
  '--locked',
  '--release',
  '-p',
  'spreadsheet-wasm',
  '--target',
  'wasm32-unknown-unknown',
]);
run(cli, [
  '--target',
  'web',
  '--typescript',
  '--out-name',
  'spreadsheet_wasm',
  '--out-dir',
  output,
  resolve(cargoTargetDir, 'wasm32-unknown-unknown/release/spreadsheet_wasm.wasm'),
]);
for (const file of ['spreadsheet_wasm.js', 'spreadsheet_wasm.d.ts', 'spreadsheet_wasm_bg.wasm'])
  if (!existsSync(resolve(output, file))) throw new Error(`Missing generated artifact ${file}`);
console.log(`Built native Rust engine WASM bindings (${version})`);
