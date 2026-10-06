import { useState, useSyncExternalStore } from 'react';
import type { DocumentStore } from '../document/store';
import type { ShortcutRegistry } from '../commands/registry';

export function AssistantPanel({
  document,
  commands,
}: {
  document: DocumentStore;
  commands?: ShortcutRegistry | undefined;
}) {
  useSyncExternalStore(commands?.subscribe ?? (() => () => {}), commands?.snapshot ?? (() => 0));
  const [open, setOpen] = useState(false);
  const [count, setCount] = useState(10);
  const tip = document.assistantSuggestion;
  const enabled = document.assistant.enabled;
  const visible = open || Boolean(tip);
  return (
    <aside className="assistant" aria-label="Умный помощник">
      <button
        type="button"
        className="assistant-toggle"
        aria-expanded={visible}
        onClick={() => {
          if (visible && tip) {
            document.assistant.dismiss(tip.id);
            document.notify();
          }
          setOpen(!visible);
        }}
      >
        <span aria-hidden="true">✦</span> Помощник
        {!enabled ? ' · выключен' : tip ? ' · подсказка' : ''}
      </button>
      {visible && (
        <div className="assistant-panel">
          <div className="assistant-heading">
            <strong>Умный помощник</strong>
            <button
              type="button"
              aria-label="Закрыть помощника"
              onClick={() => {
                if (tip) document.assistant.dismiss(tip.id);
                setOpen(false);
                document.notify();
              }}
            >
              ×
            </button>
          </div>
          <label className="assistant-setting">
            <input
              type="checkbox"
              checked={enabled}
              onChange={(e) => {
                setOpen(true);
                document.setAssistantEnabled(e.target.checked);
              }}
            />{' '}
            Подсказки и анализ действий
          </label>
          {enabled ? (
            <>
              {tip ? (
                <div className="assistant-tip" role="status">
                  <strong>{tip.title}</strong>
                  <p>{tip.description}</p>
                  {tip.source && (
                    <div className="assistant-fill">
                      <label>
                        Ячеек{' '}
                        <input
                          type="number"
                          min={1}
                          max={1000}
                          value={count}
                          onChange={(e) => setCount(Number(e.target.value))}
                        />
                      </label>
                      <button
                        type="button"
                        disabled={
                          !Number.isInteger(count) ||
                          count < 1 ||
                          count > 1000 ||
                          !document.canAutofill
                        }
                        onClick={() => {
                          setOpen(true);
                          document.applyAssistantSuggestion(tip, count);
                        }}
                      >
                        Заполнить {count}
                      </button>
                    </div>
                  )}
                  <button
                    type="button"
                    className="assistant-dismiss"
                    onClick={() => {
                      document.assistant.dismiss(tip.id);
                      document.notify();
                    }}
                  >
                    Не подсказывать это в этом листе
                  </button>
                </div>
              ) : (
                <p className="assistant-idle">
                  Замечаю повторный ввод, числовые ряды, формулы, вставку и оформление. Подсказки
                  появятся после трёх похожих действий.
                </p>
              )}
              <dl className="assistant-shortcuts">
                <div>
                  <dt>Заполнить вниз</dt>
                  <dd>Ctrl+D / Cmd+D</dd>
                </div>
                {(['copy', 'paste', 'undo', 'redo', 'bold', 'italic', 'find'] as const).map(
                  (id) => {
                    const command = commands?.commands.find((c) => c.id === id);
                    return command ? (
                      <div key={id}>
                        <dt>{command.label}</dt>
                        <dd>{commands?.description(id) || 'Не назначено'}</dd>
                      </div>
                    ) : null;
                  },
                )}
                <div>
                  <dt>Выделить диапазон</dt>
                  <dd>Shift + стрелки</dd>
                </div>
                <div>
                  <dt>Редактировать</dt>
                  <dd>F2</dd>
                </div>
              </dl>
              {commands && (
                <button type="button" onClick={() => commands.open('help')}>
                  Все горячие клавиши
                </button>
              )}
              <p className="assistant-privacy">
                Анализ выполняется в браузере. Наблюдения хранятся до смены листа; содержимое ячеек
                не сохраняется в движке.
              </p>
            </>
          ) : (
            <p className="assistant-idle">
              Помощник выключен. Наблюдения очищены, действия не анализируются. Протягивание
              диапазона остаётся доступным.
            </p>
          )}
        </div>
      )}
    </aside>
  );
}
