import { Select, SelectOption } from '../ui/Select';
import { useState } from 'react';
import { Modal } from '../sharing/Modal';
import { columnLabel } from '../grid/viewport';
import { uuidV7, type DocumentStore } from './store';
import type { FilterPredicate, FilterValue, PersonalFilter } from './filters';

const valueLabel = (value: FilterValue) =>
  value.type === 'blank'
    ? '(Пустые)'
    : value.type === 'boolean'
      ? value.value
        ? 'TRUE'
        : 'FALSE'
      : String(value.value);
function choices(
  document: DocumentStore,
  column: number,
  first: number,
  last: number,
): FilterValue[] {
  const values = new Map<string, FilterValue>();
  for (let row = first; row <= last; row++) {
    const value = document.confirmedScalar({ row, column });
    const key: FilterValue =
      !value || value.type === 'blank' || (value.type === 'text' && value.value === '')
        ? { type: 'blank' }
        : value;
    values.set(JSON.stringify(key), key);
    if (values.size > 500) break;
  }
  return [...values.values()];
}
export function ViewPanel({ document, onClose }: { document: DocumentStore; onClose: () => void }) {
  const sheet = document.sheet!;
  const existing = document.personalView.settings.filters.find((f) => f.sheet_id === sheet.id);
  const bounds = document.selectionBounds;
  const initialFirst = existing ? sheet.rows.indexOf(existing.first_row_id) : bounds.startRow;
  const initialLast = existing ? sheet.rows.indexOf(existing.last_row_id) : bounds.endRow;
  const [first, setFirst] = useState(Math.max(0, initialFirst) + 1);
  const [last, setLast] = useState(Math.max(0, initialLast) + 1);
  const [left, setLeft] = useState(
    existing ? sheet.columns.indexOf(existing.first_column_id) : bounds.startColumn,
  );
  const [right, setRight] = useState(
    existing ? sheet.columns.indexOf(existing.last_column_id) : bounds.endColumn,
  );
  const [header, setHeader] = useState(
    existing ? Boolean(existing.header_row_id) : initialFirst < initialLast,
  );
  const [headerRow, setHeaderRow] = useState(
    existing?.header_row_id ? sheet.rows.indexOf(existing.header_row_id) + 1 : first,
  );
  const [conditions, setConditions] = useState<PersonalFilter['conditions']>(
    existing?.conditions ?? [],
  );
  const [frozen, setFrozen] = useState(sheet.frozen_rows ?? 0);
  const [error, setError] = useState('');
  const [status, setStatus] = useState('');
  const busy = document.viewLoading;
  const valid =
    Number.isInteger(first) &&
    Number.isInteger(last) &&
    first >= 1 &&
    last >= first &&
    last <= sheet.rows.length &&
    left >= 0 &&
    right >= left &&
    right < sheet.columns.length &&
    (last - first + 1) * (right - left + 1) <= 100_000;
  const columns = sheet.columns
    .map((id, index) => ({ id, index }))
    .filter((c) => c.index >= left && c.index <= right);
  const change = (index: number, predicate: FilterPredicate) =>
    setConditions((previous) => previous.map((c, i) => (i === index ? { ...c, predicate } : c)));
  const save = async (reset = false) => {
    setError('');
    setStatus('');
    if (
      !reset &&
      (!valid ||
        (header && (headerRow < first || headerRow > last)) ||
        new Set(conditions.map((c) => c.column_id)).size !== conditions.length)
    ) {
      setError(
        'Проверьте границы диапазона, строку заголовка и уникальность столбцов. Предел — 100 000 ячеек.',
      );
      return;
    }
    const others = document.personalView.settings.filters.filter((f) => f.sheet_id !== sheet.id);
    const filter: PersonalFilter = {
      id: existing?.id ?? uuidV7(),
      sheet_id: sheet.id,
      first_row_id: sheet.rows[first - 1]!,
      last_row_id: sheet.rows[last - 1]!,
      first_column_id: sheet.columns[left]!,
      last_column_id: sheet.columns[right]!,
      header_row_id: header ? sheet.rows[headerRow - 1]! : null,
      conditions,
    };
    try {
      await document.saveFilters({ filters: reset ? others : [...others, filter] });
      if (reset) setConditions([]);
      setStatus(reset ? 'Личный фильтр снят.' : 'Личный фильтр сохранён.');
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : 'Не удалось сохранить фильтр.');
    }
  };
  return (
    <Modal title="Вид листа" onClose={onClose} clipboardError={document.error}>
      <p className="sharing-help">
        Закрепления общие для листа. Фильтры и строка заголовка сохраняются только для вас; скрытые
        строки остаются в книге.
      </p>
      <fieldset
        className="sharing-fieldset view-fields"
        disabled={!document.canEdit || document.pending.length > 0}
      >
        <legend>Закрепление</legend>
        <label>
          Верхних строк{' '}
          <input
            type="number"
            min={0}
            max={Math.min(1000, sheet.rows.length)}
            value={frozen}
            onChange={(e) => setFrozen(Number(e.target.value))}
          />
        </label>
        <button
          type="button"
          onClick={() => {
            if (document.setFrozenRows(frozen)) setStatus('Закрепление отправлено на сохранение.');
            else setError('Введите допустимое число строк и дождитесь сохранения книги.');
          }}
        >
          Сохранить закрепление
        </button>
      </fieldset>
      <fieldset className="sharing-fieldset view-fields" disabled={busy}>
        <legend>Личный фильтр</legend>
        <div className="view-range">
          <label>
            Первая строка{' '}
            <input
              type="number"
              min={1}
              max={sheet.rows.length}
              value={first}
              onChange={(e) => setFirst(Number(e.target.value))}
            />
          </label>
          <label>
            Последняя строка{' '}
            <input
              type="number"
              min={1}
              max={sheet.rows.length}
              value={last}
              onChange={(e) => setLast(Number(e.target.value))}
            />
          </label>
          <label>
            Первый столбец{' '}
            <Select
              aria-label="Первый столбец"
              value={left}
              onValueChange={(e) => setLeft(Number(e))}
            >
              {sheet.columns.map((id, i) => (
                <SelectOption key={id} value={i}>
                  {columnLabel(i)}
                </SelectOption>
              ))}
            </Select>
          </label>
          <label>
            Последний столбец{' '}
            <Select
              aria-label="Последний столбец"
              value={right}
              onValueChange={(e) => setRight(Number(e))}
            >
              {sheet.columns.map((id, i) => (
                <SelectOption key={id} value={i}>
                  {columnLabel(i)}
                </SelectOption>
              ))}
            </Select>
          </label>
        </div>
        <p>
          Диапазон: {columnLabel(Math.max(0, left))}
          {first}:{columnLabel(Math.max(0, right))}
          {last}
        </p>
        <label>
          <input type="checkbox" checked={header} onChange={(e) => setHeader(e.target.checked)} />{' '}
          Есть заголовок
        </label>
        {header && (
          <label>
            Строка заголовка{' '}
            <input
              type="number"
              min={first}
              max={last}
              value={headerRow}
              onChange={(e) => setHeaderRow(Number(e.target.value))}
            />
          </label>
        )}
        {conditions.map((condition, index) => {
          const values = valid
            ? choices(
                document,
                sheet.columns.indexOf(condition.column_id),
                header ? headerRow : first - 1,
                last - 1,
              )
            : [];
          const predicate = condition.predicate;
          return (
            <fieldset key={index} className="view-condition">
              <legend>Условие {index + 1} · AND</legend>
              <label>
                Столбец{' '}
                <Select
                  aria-label={`Столбец условия ${index + 1}`}
                  value={condition.column_id}
                  onValueChange={(e) =>
                    setConditions((previous) =>
                      previous.map((c, i) => (i === index ? { ...c, column_id: e } : c)),
                    )
                  }
                >
                  {columns.map((c) => (
                    <SelectOption key={c.id} value={c.id}>
                      {columnLabel(c.index)}
                    </SelectOption>
                  ))}
                </Select>
              </label>
              <label>
                Тип{' '}
                <Select
                  aria-label={`Тип условия ${index + 1}`}
                  value={predicate.kind}
                  onValueChange={(e) => {
                    const kind = e;
                    change(
                      index,
                      kind === 'text'
                        ? { kind, operator: 'contains', value: '' }
                        : kind === 'number'
                          ? { kind, operator: 'gte', value: 0 }
                          : kind === 'values'
                            ? { kind, values: values.slice(0, 500) }
                            : { kind: kind as 'blank' | 'nonblank' },
                    );
                  }}
                >
                  <SelectOption value="values" disabled={values.length > 500}>
                    Выбранные значения
                  </SelectOption>
                  <SelectOption value="blank">Пустые</SelectOption>
                  <SelectOption value="nonblank">Непустые</SelectOption>
                  <SelectOption value="text">Текст</SelectOption>
                  <SelectOption value="number">Числа</SelectOption>
                </Select>
              </label>
              {(predicate.kind === 'text' || predicate.kind === 'number') && (
                <>
                  <label>
                    Сравнение{' '}
                    <Select
                      aria-label={`Сравнение условия ${index + 1}`}
                      value={predicate.operator}
                      onValueChange={(e) =>
                        change(index, { ...predicate, operator: e } as FilterPredicate)
                      }
                    >
                      {(predicate.kind === 'text'
                        ? [
                            ['contains', 'Содержит'],
                            ['equals', 'Равно'],
                            ['starts_with', 'Начинается с'],
                          ]
                        : [
                            ['eq', '='],
                            ['ne', '≠'],
                            ['gt', '>'],
                            ['gte', '≥'],
                            ['lt', '<'],
                            ['lte', '≤'],
                          ]
                      ).map(([value, label]) => (
                        <SelectOption key={value} value={value}>
                          {label}
                        </SelectOption>
                      ))}
                    </Select>
                  </label>
                  <label>
                    Значение{' '}
                    <input
                      aria-label={`Значение условия ${index + 1}`}
                      type={predicate.kind === 'number' ? 'number' : 'text'}
                      value={predicate.value}
                      maxLength={1024}
                      onChange={(e) =>
                        change(
                          index,
                          predicate.kind === 'number'
                            ? { ...predicate, value: Number(e.target.value) }
                            : { ...predicate, value: e.target.value },
                        )
                      }
                    />
                  </label>
                </>
              )}
              {predicate.kind === 'values' && (
                <div className="view-values">
                  {values.slice(0, 500).map((value) => {
                    const key = JSON.stringify(value),
                      checked = predicate.values.some((v) => JSON.stringify(v) === key);
                    return (
                      <label key={key}>
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={() =>
                            change(index, {
                              kind: 'values',
                              values: checked
                                ? predicate.values.filter((v) => JSON.stringify(v) !== key)
                                : [...predicate.values, value],
                            })
                          }
                        />
                        {valueLabel(value)}
                      </label>
                    );
                  })}
                </div>
              )}
              <button
                type="button"
                onClick={() => setConditions((previous) => previous.filter((_, i) => i !== index))}
              >
                Удалить условие {index + 1}
              </button>
            </fieldset>
          );
        })}
        <button
          type="button"
          disabled={!valid || conditions.length >= Math.min(50, columns.length)}
          onClick={() => {
            const column = columns.find((c) => !conditions.some((p) => p.column_id === c.id));
            if (column)
              setConditions((previous) => [
                ...previous,
                { column_id: column.id, predicate: { kind: 'nonblank' } },
              ]);
          }}
        >
          Добавить условие
        </button>
        <div className="view-actions">
          <button
            type="button"
            className="sharing-primary"
            disabled={document.pending.length > 0}
            onClick={() => void save()}
          >
            Сохранить фильтр
          </button>
          <button type="button" onClick={() => void save(true)}>
            Снять фильтр листа
          </button>
        </div>
      </fieldset>
      <p className="sharing-help">
        Текст сравнивается с учётом регистра, числа — без преобразования текста. Объединение
        показывается целиком, если фильтр скрывает лишь часть его строк.
      </p>
      {document.filterState.waiting && <p role="status">Фильтр ожидает подтверждённого расчёта.</p>}
      {document.filterState.adjusted_merges > 0 && (
        <p role="status">Целиком показано объединений: {document.filterState.adjusted_merges}.</p>
      )}
      <p role="status">
        Скрыто строк: {document.filterState.hidden.size}. {status}
      </p>
      {(error || document.viewError) && <p role="alert">{error || document.viewError}</p>}
    </Modal>
  );
}
