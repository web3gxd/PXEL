import { request, type FullConfig } from '@playwright/test';

export default async function globalSetup(config: FullConfig) {
  const settings = config.projects[0]?.use;
  const baseURL = settings?.baseURL;
  const storageState = settings?.storageState;
  if (!baseURL || typeof storageState !== 'string')
    throw new Error('Missing E2E session configuration');
  const fs = (
    globalThis as unknown as {
      process: {
        getBuiltinModule(name: 'fs'): {
          mkdirSync(path: URL, options: { recursive: true; mode: number }): void;
          chmodSync(path: string, mode: number): void;
          existsSync(path: string): boolean;
          unlinkSync(path: string): void;
        };
      };
    }
  ).process.getBuiltinModule('fs');
  fs.mkdirSync(new URL('../../../.local/', import.meta.url), { recursive: true, mode: 0o700 });
  const context = await request.newContext({ baseURL });
  let issued = false;
  try {
    const response = await context.post('/api/dev/session', {
      headers: { Origin: new URL(baseURL).origin },
      timeout: 10000,
    });
    if (!response.ok()) throw new Error(`E2E session setup failed (${response.status()})`);
    issued = true;
    await context.storageState({ path: storageState });
    fs.chmodSync(storageState, 0o600);
  } catch (error) {
    if (issued) {
      await context
        .delete('/api/dev/session', {
          headers: { Origin: new URL(baseURL).origin },
          timeout: 10000,
        })
        .catch(() => undefined);
    }
    if (fs.existsSync(storageState)) fs.unlinkSync(storageState);
    throw error;
  } finally {
    await context.dispose();
  }
}
