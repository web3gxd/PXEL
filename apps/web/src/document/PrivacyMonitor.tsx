import { useEffect, useRef } from 'react';
import type { DocumentStore } from './store';
import { analyzePrivacy, readPrivacy, type PrivacyPolicy } from './privacy';

/** Fixed-head, owner-only analysis; guest projections never load original values. */
export function PrivacyMonitor({ document }: { document: DocumentStore }) {
  const cached = useRef<{ id: string; revision: number | undefined; policy: PrivacyPolicy }>(
    undefined,
  );
  const id = document.projection?.document_id;
  const version = document.projection?.version;
  const revision = document.projection?.capabilities?.disclosure_stamp.book_revision;
  const owner = document.canShare && !document.isGuest;
  useEffect(() => {
    if (!id || !version || !owner) return;
    const abort = new AbortController();
    let active = true;
    document.privacyAnalysisPending = true;
    document.privacyAnalysisError = '';
    document.notify();
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          const policy =
            cached.current?.id === id && cached.current.revision === revision
              ? cached.current.policy
              : await readPrivacy(id, abort.signal);
          if (!active) return;
          cached.current = { id, revision, policy };
          document.privacyPolicy = policy;
          if (!policy.rules.enabled || !policy.rules.types.length) {
            document.privacyReport = undefined;
            document.privacyFindings = [];
            document.refreshPrivacyMask(undefined);
            return;
          }
          const report = await analyzePrivacy(id, version);
          if (
            !active ||
            document.projection?.version !== version ||
            report.projection.document_id !== id ||
            report.policy_revision !== policy.revision
          )
            return;
          document.privacyReport = report;
          document.privacyFindings = report.findings;
          document.refreshPrivacyMask(report.projection);
        } catch (error) {
          if (active)
            document.privacyAnalysisError =
              error instanceof Error ? error.message : 'Распознавание недоступно.';
        } finally {
          if (active) {
            document.privacyAnalysisPending = false;
            document.notify();
          }
        }
      })();
    }, 200);
    return () => {
      active = false;
      abort.abort();
      window.clearTimeout(timer);
    };
  }, [document, id, version, revision, owner]);
  return null;
}
