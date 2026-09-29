type FormulaCopy = {
  source: string;
  fromRow: number;
  fromColumn: number;
  toRow: number;
  toColumn: number;
};

export function copyFormulaInWorker(input: FormulaCopy): Promise<string> {
  return new Promise((resolve, reject) => {
    let worker: Worker;
    try {
      worker = new Worker(new URL('./formula-copy.worker.ts', import.meta.url), { type: 'module' });
    } catch {
      reject(new Error('Formula copy worker unavailable'));
      return;
    }
    const timer = setTimeout(() => finish(new Error('Formula copy timed out')), 5000);
    const finish = (error?: Error, text?: string) => {
      clearTimeout(timer);
      worker.terminate();
      if (error) reject(error);
      else if (text !== undefined) resolve(text);
      else reject(new Error('Formula copy failed'));
    };
    worker.onmessage = (event: MessageEvent<{ text?: string; error?: string }>) => {
      if (typeof event.data.text === 'string') finish(undefined, event.data.text);
      else finish(new Error('Formula copy rejected'));
    };
    worker.onerror = () => finish(new Error('Formula copy worker failed'));
    worker.postMessage(input);
  });
}
