import { useRef, useState } from 'react';
import type { DocumentStore } from '../document/store';
import { Modal } from '../sharing/Modal';
import { Select, SelectOption } from './Select';

export type CellsPanelKind =
  'insert' | 'delete' | 'defaultWidth' | 'rename' | 'moveCopy' | 'tabColor' | 'showSheet';
const titles: Record<CellsPanelKind, string> = {
  insert: 'Вставить ячейки',
  delete: 'Удалить ячейки',
  defaultWidth: 'Ширина по умолчанию',
  rename: 'Переименовать лист',
  moveCopy: 'Переместить или скопировать лист',
  tabColor: 'Цвет вкладки',
  showSheet: 'Показать лист',
};
export function CellsPanel({
  document,
  kind,
  onClose,
}: {
  document: DocumentStore;
  kind: CellsPanelKind;
  onClose: () => void;
}) {
  const first = useRef<HTMLElement>(null);
  const sheet = document.sheet!;
  const [name, setName] = useState(sheet.name);
  const [width, setWidth] = useState(String(sheet.default_column_width ?? 112));
  const [axis, setAxis] = useState<'row' | 'column'>('row');
  const [copy, setCopy] = useState(false);
  const [index, setIndex] = useState(
    String(document.projection!.sheets.findIndex((s) => s.id === sheet.id)),
  );
  const [copyName, setCopyName] = useState(() => {
    const names = new Set(document.projection!.sheets.map((s) => s.name.toLowerCase()));
    let number = 1,
      candidate = '';
    do {
      const suffix = ` (${number++})`;
      candidate =
        Array.from(sheet.name)
          .slice(0, 31 - suffix.length)
          .join('') + suffix;
    } while (names.has(candidate.toLowerCase()));
    return candidate;
  });
  const [color, setColor] = useState(
    `#${(sheet.tab_color ?? 0x2f805b).toString(16).padStart(6, '0')}`,
  );
  const [shown, setShown] = useState(document.projection!.sheets.find((s) => s.hidden)?.id ?? '');
  const [error, setError] = useState('');
  const [stamp] = useState({ sheet: sheet.id, version: document.projection!.version });
  const submit = (resetColor = false) => {
    if (document.sheet?.id !== stamp.sheet || document.projection?.version !== stamp.version) {
      setError('Книга изменилась. Закройте окно и откройте его заново.');
      return;
    }
    if (!document.canChangeCells) {
      setError('Завершите ввод и дождитесь сохранения книги.');
      return;
    }
    const ok =
      kind === 'insert' || kind === 'delete'
        ? document.shiftCells(axis, kind === 'insert')
        : kind === 'rename'
          ? document.renameSheet(name)
          : kind === 'defaultWidth'
            ? document.setDefaultColumnWidth(Number(width))
            : kind === 'moveCopy'
              ? document.moveOrCopySheet(Number(index), copy, copyName)
              : kind === 'tabColor'
                ? document.setTabColor(resetColor ? null : parseInt(color.slice(1), 16))
                : document.setSheetHidden(false, shown);
    if (ok) onClose();
    else setError(document.error || 'Не удалось выполнить действие.');
  };
  return (
    <Modal title={titles[kind]} onClose={onClose} initialFocus={first}>
      <form
        ref={(element) => {
          first.current =
            element?.querySelector<HTMLElement>('button[role="combobox"], input') ?? null;
        }}
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        {(kind === 'insert' || kind === 'delete') && (
          <>
            <label>
              <span>Сдвиг ячеек</span>
              <Select
                aria-label="Сдвиг ячеек"
                value={axis}
                onValueChange={(value) => setAxis(value as 'row' | 'column')}
              >
                <SelectOption value="row">{kind === 'insert' ? 'Вниз' : 'Вверх'}</SelectOption>
                <SelectOption value="column">{kind === 'insert' ? 'Вправо' : 'Влево'}</SelectOption>
              </Select>
            </label>
            <p className="sharing-intro">
              Сдвигается полоса от выделения до края листа. До 1000 затронутых ячеек. Объединения и
              неподдерживаемые зависимости формул отклоняют действие целиком.
            </p>
          </>
        )}
        {kind === 'rename' && (
          <label>
            <span>Имя листа</span>
            <input value={name} onChange={(event) => setName(event.target.value)} maxLength={31} />
          </label>
        )}
        {kind === 'defaultWidth' && (
          <>
            <label>
              <span>Ширина по умолчанию в пикселях</span>
              <input
                type="number"
                min={24}
                max={2000}
                step={1}
                value={width}
                onChange={(event) => setWidth(event.target.value)}
              />
            </label>
            <p className="sharing-intro">
              Применяется к столбцам без собственной ширины. Диапазон: 24–2000 px.
            </p>
          </>
        )}
        {kind === 'moveCopy' && (
          <>
            <label>
              <span>Позиция листа</span>
              <Select aria-label="Позиция листа" value={index} onValueChange={setIndex}>
                {document.projection!.sheets.map((s, i) => (
                  <SelectOption key={s.id} value={i}>
                    {i + 1} — {document.displaySheetName(s.id)}
                  </SelectOption>
                ))}
                {copy && (
                  <SelectOption value={document.projection!.sheets.length}>
                    В конец книги
                  </SelectOption>
                )}
              </Select>
            </label>
            <label>
              <input
                type="checkbox"
                checked={copy}
                onChange={(event) => {
                  setCopy(event.target.checked);
                  if (!event.target.checked)
                    setIndex(
                      String(Math.min(Number(index), document.projection!.sheets.length - 1)),
                    );
                }}
              />{' '}
              Создать копию
            </label>
            {copy && (
              <label>
                <span>Имя копии</span>
                <input
                  value={copyName}
                  maxLength={31}
                  onChange={(event) => setCopyName(event.target.value)}
                />
              </label>
            )}
          </>
        )}
        {kind === 'tabColor' && (
          <label>
            <span>Цвет вкладки</span>
            <input type="color" value={color} onChange={(event) => setColor(event.target.value)} />
          </label>
        )}
        {kind === 'showSheet' && (
          <label>
            <span>Скрытый лист</span>
            <Select aria-label="Скрытый лист" value={shown} onValueChange={setShown}>
              {document
                .projection!.sheets.filter((s) => s.hidden)
                .map((s) => (
                  <SelectOption key={s.id} value={s.id}>
                    {document.displaySheetName(s.id)}
                  </SelectOption>
                ))}
            </Select>
          </label>
        )}
        {error && <p role="alert">{error}</p>}
        <footer>
          <button type="button" className="markdown-button-cancel" onClick={onClose}>
            Отмена
          </button>
          {kind === 'tabColor' && (
            <button type="button" disabled={!document.canChangeCells} onClick={() => submit(true)}>
              Без цвета
            </button>
          )}
          <button
            type="submit"
            disabled={!document.canChangeCells || (kind === 'showSheet' && !shown)}
          >
            {kind === 'insert' ? 'Вставить' : kind === 'delete' ? 'Удалить' : 'Применить'}
          </button>
        </footer>
      </form>
    </Modal>
  );
}
