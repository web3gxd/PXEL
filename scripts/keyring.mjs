import { spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, statSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

// Local development/test setup only. Production must provision its own keyring.
export function protectLocalPath(path, directory = false) {
  if (process.platform !== 'win32') {
    chmodSync(path, directory ? 0o700 : 0o600);
    return;
  }
  const script =
    "$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=Get-Acl -LiteralPath $env:PXEL_PRIVATE_SETUP_PATH; $acl.SetAccessRuleProtection($true,$false); foreach($entry in @($acl.Access)){[void]$acl.RemoveAccessRuleSpecific($entry)}; $acl.SetOwner($sid); $flags=if($env:PXEL_PRIVATE_SETUP_DIRECTORY -eq '1'){'ContainerInherit,ObjectInherit'}else{'None'}; $rule=[System.Security.AccessControl.FileSystemAccessRule]::new($sid,'FullControl',$flags,'None','Allow'); $acl.AddAccessRule($rule); Set-Acl -LiteralPath $env:PXEL_PRIVATE_SETUP_PATH -AclObject $acl";
  const env = {
    ...process.env,
    PXEL_PRIVATE_SETUP_PATH: resolve(path),
    PXEL_PRIVATE_SETUP_DIRECTORY: directory ? '1' : '0',
  };
  delete env.PSModulePath;
  const result = spawnSync(
    'powershell.exe',
    ['-NoProfile', '-NonInteractive', '-Command', script],
    {
      env,
      windowsHide: true,
      stdio: 'ignore',
      timeout: 10000,
    },
  );
  if (result.error || result.status !== 0) throw new Error('Private local file setup failed');
}

export function ensureDevelopmentKeyring(path = resolve('.local/keys/keyring.json')) {
  if (process.env.PXEL_KEYRING_FILE || process.env.PXEL_KEYRING_JSON) return {};
  const parent = dirname(path);
  mkdirSync(parent, { recursive: true, mode: 0o700 });
  protectLocalPath(parent, true);
  if (!existsSync(path)) {
    const material = randomBytes(32);
    try {
      writeFileSync(
        path,
        JSON.stringify({
          schema_version: 1,
          active_kek_id: 'local-dev-v1',
          keys: [{ id: 'local-dev-v1', key_hex: material.toString('hex') }],
        }),
        { flag: 'wx', mode: 0o600 },
      );
      protectLocalPath(path);
    } finally {
      material.fill(0);
    }
  }
  if (!statSync(path).isFile() || statSync(path).nlink !== 1)
    throw new Error('Local keyring must be a regular file without hard links');
  return { PXEL_KEYRING_FILE: resolve(path) };
}
