import { request, type FullConfig } from '@playwright/test';

export default async function globalTeardown(config: FullConfig) {
  const settings = config.projects[0]?.use;
  const baseURL = settings?.baseURL;
  const storageState = settings?.storageState;
  if (!baseURL || typeof storageState !== 'string') return;
  // Node 24 is pinned. Keep this small runner boundary independent of browser TS globals.
  const fs = (
    globalThis as unknown as {
      process: {
        getBuiltinModule(name: 'fs'): {
          existsSync(path: string): boolean;
          unlinkSync(path: string): void;
        };
      };
    }
  ).process.getBuiltinModule('fs');
  if (!fs.existsSync(storageState)) return;
  try {
    const context = await request.newContext({ baseURL, storageState });
    try {
      const response = await context.delete('/api/dev/session', {
        headers: { Origin: new URL(baseURL).origin },
        timeout: 10000,
      });
      // An expired session already occupies no active-session capacity.
      if (!response.ok() && response.status() !== 401)
        throw new Error(`E2E session cleanup failed (${response.status()})`);
    } finally {
      await context.dispose();
    }
  } finally {
    fs.unlinkSync(storageState);
  }
}
