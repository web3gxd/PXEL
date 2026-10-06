import { useEffect } from 'react';
import { boundedJson } from '../sharing/api';
import { decodeHardcodeReport } from './HardcodePanel';
import type { DocumentStore } from './store';
export function HardcodeMonitor({ document }: { document: DocumentStore }) {
  const id = document.projection?.document_id,
    version = document.projection?.version,
    guest = document.isGuest;
  useEffect(() => {
    if (
      !id ||
      !version ||
      guest ||
      [...document.confirmed.values()].filter((value) => value.type === 'formula').length < 100
    )
      return;
    const controller = new AbortController();
    const timer = setTimeout(() => {
      void (async () => {
        try {
          const response = await fetch(`/api/documents/${id}/diagnostics`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ schema_version: 1, document_version: version }),
            signal: controller.signal,
          });
          if (!response.ok) return;
          const report = decodeHardcodeReport(await boundedJson(response, 512 * 1024));
          if (
            !controller.signal.aborted &&
            document.projection?.document_id === id &&
            document.projection.version === report.document_version
          ) {
            document.hardcodeReport = report;
            document.notify();
          }
        } catch {
          /* Optional diagnostics never change or block the confirmed workbook. */
        }
      })();
    }, 600);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [document, id, version, guest]);
  return null;
}
