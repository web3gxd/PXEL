import { useEffect, useRef, useState } from 'react';
import { Modal } from '../sharing/Modal';
import { boundedJson } from '../sharing/api';
import { columnLabel } from '../grid/viewport';
import { decodeProfile, dependencyChain, type ProfileAddress, type ProfileReport } from './profile';
import type { DocumentStore } from './store';
const label = (address: ProfileAddress) =>
  `${address.sheet_name}!${columnLabel(address.column)}${address.row + 1}`;
const duration = (microseconds: number) => `${(microseconds / 1000).toFixed(3)} мс`;
export function ProfilePanel({
  document,
  onClose,
}: {
  document: DocumentStore;
  onClose: () => void;
}) {
  const [captured] = useState({
    id: document.projection!.document_id,
    version: document.projection!.version,
  });
  const [report, setReport] = useState<ProfileReport>();
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState('');
  const [selected, setSelected] = useState(0);
  const request = useRef<AbortController>(null);
  const run = async (version: string) => {
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    setBusy(true);
    setError('');
    try {
      const response = await fetch(`/api/documents/${captured.id}/profile`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ schema_version: 1, document_version: version }),
        signal: controller.signal,
      });
      if (response.status === 422) {
        const detail = (await boundedJson(response, 1024)) as { code?: unknown };
        if (detail.code === 'profile_context_missing')
          throw new Error(
            'У книги отсутствует сохранённый контекст расчёта. Откройте книгу обычным способом и повторите анализ.',
          );
      }
      if (!response.ok)
        throw new Error(
          response.status === 409
            ? 'Книга изменилась. Запустите анализ актуальной версии.'
            : response.status === 403
              ? 'Профайлер доступен владельцу книги.'
              : 'Профайлер недоступен или превысил бюджет расчёта.',
        );
      const value = decodeProfile(await boundedJson(response, 16 * 1024 * 1024));
      if (!controller.signal.aborted) {
        setReport(value);
        setSelected(0);
      }
    } catch (failure) {
      if (!controller.signal.aborted)
        setError(failure instanceof Error ? failure.message : 'Анализ недоступен.');
    } finally {
      if (!controller.signal.aborted) setBusy(false);
    }
  };
  useEffect(() => {
    void run(captured.version);
    return () => request.current?.abort();
  }, [captured]);
  const chain = report ? dependencyChain(report, selected) : [];
  const positions = new Map(
    chain.map((id, index) => [
      id,
      { x: 105 + (index % 3) * 220, y: 40 + Math.floor(index / 3) * 76 },
    ]),
  );
  return (
    <Modal title="Профайлер формул" onClose={onClose} clipboardError={document.error}>
      <p>
        Анализ использует отдельную проекцию на зафиксированной версии и контексте расчёта.
        Измерения зависят от нагрузки сервера; книга и история не изменяются.
      </p>
      <button
        type="button"
        disabled={
          busy || document.pending.length > 0 || document.projection?.document_id !== captured.id
        }
        onClick={() => {
          void run(document.projection!.version);
        }}
      >
        Повторить анализ
      </button>
      {busy && <p role="status">Измерение формул…</p>}
      {error && <p role="alert">{error}</p>}
      {report && (
        <>
          <p role="status">
            Версия {report.document_version}. Эпоха расчёта{' '}
            {report.calculation_epoch ?? 'не задана'}. Формул: {report.formula_count}.
          </p>
          {document.projection?.version !== report.document_version && (
            <p role="alert">Отчёт устарел после изменения книги.</p>
          )}
          <dl className="profile-summary">
            <div>
              <dt>Подготовка графа</dt>
              <dd>{duration(report.preparation_us)}</dd>
            </div>
            <div>
              <dt>Расчёт</dt>
              <dd>{duration(report.calculation_us)}</dd>
            </div>
            <div>
              <dt>Общее время</dt>
              <dd>{duration(report.total_us)}</dd>
            </div>
            <div>
              <dt>Работа</dt>
              <dd>
                {report.preparation_work + report.graph_work} граф / {report.evaluation_work}{' '}
                вычисления
              </dd>
            </div>
            <div>
              <dt>Результаты</dt>
              <dd>{report.result_bytes} байт</dd>
            </div>
          </dl>
          <h3>Медленные формулы</h3>
          {report.formula_rows_truncated && <p>Список усечён до 1000 формул.</p>}
          <div className="profile-scroll">
            <table className="profile-table">
              <thead>
                <tr>
                  <th>Ячейка</th>
                  <th>Время</th>
                  <th>Работа</th>
                  <th>Временная полоса</th>
                </tr>
              </thead>
              <tbody>
                {report.formulas.map((formula) => (
                  <tr
                    key={`${formula.address.sheet_id}/${formula.address.row_id}/${formula.address.column_id}`}
                  >
                    <td>
                      <button
                        type="button"
                        onClick={() => {
                          const index = report.graph.nodes.findIndex(
                            (node) =>
                              node.address.sheet_id === formula.address.sheet_id &&
                              node.address.row_id === formula.address.row_id &&
                              node.address.column_id === formula.address.column_id,
                          );
                          if (index >= 0) setSelected(index);
                          else setError('Цепочка этой формулы не вошла в усечённый граф.');
                        }}
                      >
                        {label(formula.address)}
                      </button>
                      {formula.cycle_or_blocked && <span> · цикл или зависимость от цикла</span>}
                    </td>
                    <td>{duration(formula.duration_us)}</td>
                    <td>{formula.work}</td>
                    <td>
                      <div className="profile-time" aria-hidden="true">
                        <span
                          style={{
                            marginLeft: `${(100 * formula.start_us) / Math.max(1, report.total_us)}%`,
                            width: `${(100 * formula.duration_us) / Math.max(1, report.total_us)}%`,
                          }}
                        />
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <h3>Потенциальные зависимости</h3>
          <p>
            Статический граф учитывает обе условные ветви. Зависимость не означает, что ветвь была
            вычислена.
          </p>
          {report.graph.truncated && (
            <p role="status">Граф усечён: максимум 500 узлов и 2000 рёбер.</p>
          )}
          {!!chain.length && (
            <>
              <div className="profile-graph" role="region" aria-label="Граф выбранной цепочки">
                <svg
                  viewBox={`0 0 650 ${Math.ceil(chain.length / 3) * 76 + 20}`}
                  aria-label="Интерактивные узлы зависимостей"
                  role="group"
                >
                  {report.graph.edges
                    .filter(([from, to]) => positions.has(from) && positions.has(to))
                    .map(([from, to]) => (
                      <path
                        key={`${from}/${to}`}
                        d={`M${positions.get(from)!.x} ${positions.get(from)!.y} L${positions.get(to)!.x} ${positions.get(to)!.y}`}
                        className="profile-edge"
                      />
                    ))}
                  {chain.map((id) => (
                    <g
                      key={id}
                      transform={`translate(${positions.get(id)!.x},${positions.get(id)!.y})`}
                      role="button"
                      tabIndex={0}
                      aria-label={`Выбрать цепочку ${label(report.graph.nodes[id]!.address)}`}
                      onClick={() => setSelected(id)}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter' || event.key === ' ') {
                          event.preventDefault();
                          setSelected(id);
                        }
                      }}
                    >
                      <rect
                        x={-98}
                        y={-20}
                        width={196}
                        height={40}
                        rx={8}
                        className={id === selected ? 'profile-node-selected' : 'profile-node'}
                      />
                      <text textAnchor="middle" dy={5}>
                        {columnLabel(report.graph.nodes[id]!.address.column)}
                        {report.graph.nodes[id]!.address.row + 1}
                      </text>
                    </g>
                  ))}
                </svg>
              </div>
              <details>
                <summary>Текстовое представление цепочки</summary>
                <ul>
                  {chain.map((id) => (
                    <li key={id}>
                      <button type="button" onClick={() => setSelected(id)}>
                        {label(report.graph.nodes[id]!.address)}
                      </button>{' '}
                      —{' '}
                      {report.graph.edges
                        .filter(([from]) => from === id)
                        .map(([, to]) => label(report.graph.nodes[to]!.address))
                        .join(', ') || 'нет потенциальных зависимостей'}
                    </li>
                  ))}
                </ul>
              </details>
            </>
          )}
        </>
      )}
    </Modal>
  );
}
