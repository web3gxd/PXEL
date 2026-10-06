import { defineConfig } from 'vite';

const runtimeEnvironment = (
  globalThis as unknown as { process: { env: Record<string, string | undefined> } }
).process.env;
const apiTarget = runtimeEnvironment.PXEL_API_TARGET ?? 'http://127.0.0.1:8080';
const webPort = Number(runtimeEnvironment.PXEL_WEB_PORT ?? 5173);

export default defineConfig({
  resolve: { conditions: ['worker'] },
  server: {
    port: webPort,
    strictPort: true,
    proxy: {
      '/api': { target: apiTarget, ws: true },
      '/healthz': apiTarget,
      '/readyz': apiTarget,
    },
  },
  worker: { format: 'es' },
  build: { target: 'es2022', sourcemap: false, assetsInlineLimit: 0 },
});
