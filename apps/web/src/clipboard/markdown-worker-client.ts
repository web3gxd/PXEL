import type { MarkdownDocument } from './markdown';
export function parseMarkdownInWorker(text: string): Promise<MarkdownDocument> {
  if (new TextEncoder().encode(text).length > 16 * 1024 * 1024)
    return Promise.reject(new Error('Размер Markdown превышает 16 МиБ.'));
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL('./markdown.worker.ts', import.meta.url), { type: 'module' });
    const timer = setTimeout(() => {
      worker.terminate();
      reject(new Error('Разбор Markdown занял слишком много времени.'));
    }, 10000);
    worker.onmessage = (
      event: MessageEvent<{ id: number; error?: string; result?: MarkdownDocument }>,
    ) => {
      if (event.data.id !== 1) return;
      clearTimeout(timer);
      worker.terminate();
      if (event.data.error || !event.data.result)
        reject(new Error('Не удалось разобрать Markdown.'));
      else resolve(event.data.result);
    };
    worker.onerror = () => {
      clearTimeout(timer);
      worker.terminate();
      reject(new Error('Не удалось разобрать Markdown.'));
    };
    worker.postMessage({ id: 1, text });
  });
}
