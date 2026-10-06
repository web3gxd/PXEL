import { parseMarkdown } from './markdown';

type Scope = {
  onmessage: ((event: MessageEvent<{ id: number; text: string }>) => void) | null;
  postMessage(value: unknown): void;
};
const scope = globalThis as unknown as Scope;
scope.onmessage = (event) => {
  try {
    scope.postMessage({ id: event.data.id, result: parseMarkdown(event.data.text) });
  } catch {
    scope.postMessage({ id: event.data.id, error: 'invalid-markdown' });
  }
};
