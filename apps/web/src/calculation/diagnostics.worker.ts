import init, { diagnoseFormula } from '../generated/wasm/spreadsheet_wasm';
import wasmUrl from '../generated/wasm/spreadsheet_wasm_bg.wasm?url';
const scope = globalThis as unknown as {
  onmessage:
    ((event: MessageEvent<{ source: string; row: number; column: number }>) => void) | null;
  postMessage: (reply: { syntax?: unknown; error?: string }) => void;
};
const ready = init({ module_or_path: wasmUrl });
scope.onmessage = async ({ data }) => {
  try {
    await ready;
    scope.postMessage({ syntax: JSON.parse(diagnoseFormula(data.source, data.row, data.column)) });
  } catch {
    scope.postMessage({ error: 'InvalidFormulaDiagnostics' });
  }
};
