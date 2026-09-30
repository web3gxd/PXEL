import init, { WasmWorkbook } from '../generated/wasm/spreadsheet_wasm';
import wasmUrl from '../generated/wasm/spreadsheet_wasm_bg.wasm?url';
import type { Reply, Request, PreviewResult } from './protocol';

const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<Request>) => void) | null;
  postMessage: (reply: Reply) => void;
};
const ready = init({ module_or_path: wasmUrl });
let workbook: WasmWorkbook | undefined;
let serial = Promise.resolve();
scope.onmessage = (event) => {
  serial = serial.then(async () => {
    const request = event.data;
    let phase: 'init' | 'apply' | 'patch' | 'move' | 'calculate' = request.kind;
    try {
      await ready;
      if (request.kind === 'init') {
        workbook?.free();
        workbook = new WasmWorkbook(JSON.stringify(request.data));
      } else if (request.kind === 'patch') {
        if (!workbook) throw new Error('Uninitialized');
        const result = JSON.parse(workbook.patch(JSON.stringify(request.data))) as PreviewResult;
        scope.postMessage({ id: request.id, result });
        return;
      } else if (request.kind === 'move') {
        if (!workbook) throw new Error('Uninitialized');
        const result = JSON.parse(
          workbook.move_preview(JSON.stringify(request.data)),
        ) as PreviewResult;
        scope.postMessage({ id: request.id, result });
        return;
      } else {
        if (!workbook) throw new Error('Uninitialized');
        workbook.apply(JSON.stringify(request.data));
      }
      if (!workbook) throw new Error('Uninitialized');
      phase = 'calculate';
      const result = JSON.parse(workbook.calculate()) as PreviewResult;
      scope.postMessage({ id: request.id, result });
    } catch (error) {
      let code = 'WasmFailure';
      if (typeof error === 'string') {
        try {
          const parsed = JSON.parse(error) as { code?: string };
          if (
            parsed.code &&
            /^(InvalidJson|LimitExceeded|InvalidIdentity|StaleRevision|UnsupportedProfile|UnsupportedMoveDependency|InvalidFormula|InvalidProjection|CalculationLimitExceeded|Cancelled)$/.test(
              parsed.code,
            )
          )
            code = parsed.code;
        } catch {
          /* Never disclose input or exception text. */
        }
      }
      scope.postMessage({ id: request.id, error: code, phase });
    }
  });
};
