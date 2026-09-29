import init, { copyFormulaText } from '../generated/wasm/spreadsheet_wasm';
import wasmUrl from '../generated/wasm/spreadsheet_wasm_bg.wasm?url';

type Request = {
  source: string;
  fromRow: number;
  fromColumn: number;
  toRow: number;
  toColumn: number;
};
type Reply = { text?: string; error?: 'InvalidFormulaCopy' };
const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<Request>) => void) | null;
  postMessage: (reply: Reply) => void;
};
const ready = init({ module_or_path: wasmUrl });
scope.onmessage = async (event) => {
  try {
    await ready;
    const { source, fromRow, fromColumn, toRow, toColumn } = event.data;
    scope.postMessage({ text: copyFormulaText(source, fromRow, fromColumn, toRow, toColumn) });
  } catch {
    scope.postMessage({ error: 'InvalidFormulaCopy' });
  }
};
