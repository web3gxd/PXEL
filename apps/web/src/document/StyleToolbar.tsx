import { useEffect, useRef, useState } from 'react';
import type { DocumentStore } from './store';
import {
  colorCss,
  DEFAULT_STYLE,
  type Border,
  type CellStyle,
  type Rgb,
  type StyleChange,
} from './styles';

const color = (text: string): Rgb => [
  parseInt(text.slice(1, 3), 16),
  parseInt(text.slice(3, 5), 16),
  parseInt(text.slice(5, 7), 16),
];
const formats: { value: CellStyle['number_format']; label: string }[] = [
  { value: 'general', label: 'Общий' },
  { value: 'integer', label: 'Целое' },
  { value: 'decimal2', label: '0.00' },
  { value: 'percent0', label: '0%' },
  { value: 'percent2', label: '0.00%' },
  { value: 'scientific2', label: '1.00E+0' },
  { value: 'date_iso', label: 'ГГГГ-ММ-ДД' },
];
export function StyleToolbar({ document, online }: { document: DocumentStore; online: boolean }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [expanded, setExpanded] = useState(false);
  useEffect(() => {
    if (expanded) dialog.current?.showModal();
  }, [expanded]);
  const style = document.style(document.selected),
    disabled = !online || !document.canFormat;
  const send = (change: StyleChange) => document.setStyle([change]);
  const toggle = (property: 'font_bold' | 'font_italic' | 'font_strike' | 'wrap') =>
    send({ property, value: !style[property] });
  return (
    <>
      <div className="style-toolbar">
        <select
          className="style-select style-font"
          aria-label="Шрифт"
          value={style.font_family}
          disabled={disabled}
          onChange={(event) => send({ property: 'font_family', value: event.target.value })}
        >
          {[
            ...new Set(['Arial', 'Calibri', 'Times New Roman', 'Courier New', style.font_family]),
          ].map((font) => (
            <option key={font}>{font}</option>
          ))}
        </select>
        <select
          className="style-select style-size"
          aria-label="Размер шрифта"
          value={style.font_size_hundredth_points}
          disabled={disabled}
          onChange={(event) =>
            send({ property: 'font_size_hundredth_points', value: Number(event.target.value) })
          }
        >
          {[
            ...new Set([
              800,
              1000,
              1100,
              1200,
              1400,
              1800,
              2400,
              3600,
              style.font_size_hundredth_points,
            ]),
          ]
            .sort((a, b) => a - b)
            .map((size) => (
              <option value={size} key={size}>
                {size / 100}
              </option>
            ))}
        </select>
        <button
          type="button"
          className="tool"
          aria-label="Полужирный"
          aria-pressed={style.font_bold}
          title="Полужирный"
          disabled={disabled}
          onClick={() => toggle('font_bold')}
        >
          <strong>B</strong>
        </button>
        <button
          type="button"
          className="tool"
          aria-label="Курсив"
          aria-pressed={style.font_italic}
          title="Курсив"
          disabled={disabled}
          onClick={() => toggle('font_italic')}
        >
          <em>I</em>
        </button>
        <label className="style-color" title="Цвет текста">
          <span aria-hidden="true">A</span>
          <input
            type="color"
            aria-label="Цвет текста"
            value={colorCss(style.font_color)}
            disabled={disabled}
            onChange={(event) => send({ property: 'font_color', value: color(event.target.value) })}
          />
        </label>
        <label className="style-color" title="Цвет заливки">
          <span aria-hidden="true">▧</span>
          <input
            type="color"
            aria-label="Цвет заливки"
            value={colorCss(style.fill)}
            disabled={disabled}
            onChange={(event) => send({ property: 'fill', value: color(event.target.value) })}
          />
        </label>
        <select
          className="style-select style-alignment"
          aria-label="Выравнивание"
          value={style.horizontal}
          disabled={disabled}
          onChange={(event) =>
            send({ property: 'horizontal', value: event.target.value as CellStyle['horizontal'] })
          }
        >
          <option value="general">Авто</option>
          <option value="left">Слева</option>
          <option value="center">По центру</option>
          <option value="right">Справа</option>
        </select>
        <select
          className="style-select style-format"
          aria-label="Числовой формат"
          value={style.number_format}
          disabled={disabled}
          onChange={(event) =>
            send({
              property: 'number_format',
              value: event.target.value as CellStyle['number_format'],
            })
          }
        >
          {formats.map(({ value, label }) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
        <button
          type="button"
          className="tool"
          aria-label="Другие параметры оформления"
          title="Другие параметры оформления"
          disabled={disabled}
          onClick={() => setExpanded(true)}
        >
          ⋯
        </button>
      </div>
      <dialog
        className="style-dialog"
        ref={dialog}
        aria-labelledby={expanded ? 'style-dialog-title' : undefined}
        onClose={() => setExpanded(false)}
      >
        {expanded && (
          <>
            <div className="style-dialog-heading">
              <h2 id="style-dialog-title">Оформление ячеек</h2>
              <button
                type="button"
                className="tool"
                aria-label="Закрыть оформление"
                onClick={() => dialog.current?.close()}
              >
                ×
              </button>
            </div>
            <p>Параметры применяются к выделенному диапазону.</p>
            <div className="style-options">
              <label>
                Название шрифта
                <input
                  key={`${document.sheet?.id}/${document.selected.row}/${document.selected.column}/${style.font_family}`}
                  defaultValue={style.font_family}
                  maxLength={128}
                  disabled={disabled}
                  onBlur={(event) => {
                    if (event.target.value !== style.font_family)
                      send({ property: 'font_family', value: event.target.value });
                  }}
                />
              </label>
              <label>
                Размер, пт
                <input
                  type="number"
                  min="1"
                  max="200"
                  step="0.01"
                  key={style.font_size_hundredth_points}
                  defaultValue={style.font_size_hundredth_points / 100}
                  disabled={disabled}
                  onBlur={(event) => {
                    const size = Math.round(Number(event.target.value) * 100);
                    if (size !== style.font_size_hundredth_points)
                      send({ property: 'font_size_hundredth_points', value: size });
                  }}
                />
              </label>
              <label>
                Подчёркивание
                <select
                  value={style.font_underline}
                  disabled={disabled}
                  onChange={(event) =>
                    send({
                      property: 'font_underline',
                      value: event.target.value as CellStyle['font_underline'],
                    })
                  }
                >
                  <option value="none">Нет</option>
                  <option value="single">Одинарное</option>
                  <option value="double">Двойное</option>
                </select>
              </label>
              <label>
                По вертикали
                <select
                  value={style.vertical}
                  disabled={disabled}
                  onChange={(event) =>
                    send({
                      property: 'vertical',
                      value: event.target.value as CellStyle['vertical'],
                    })
                  }
                >
                  <option value="top">Сверху</option>
                  <option value="center">По центру</option>
                  <option value="bottom">Снизу</option>
                </select>
              </label>
              <label className="style-check">
                <input
                  type="checkbox"
                  checked={style.font_strike}
                  disabled={disabled}
                  onChange={() => toggle('font_strike')}
                />
                Зачёркнутый
              </label>
              <label className="style-check">
                <input
                  type="checkbox"
                  checked={style.wrap}
                  disabled={disabled}
                  onChange={() => toggle('wrap')}
                />
                Перенос текста
              </label>
              {(['top', 'right', 'bottom', 'left'] as const).map((edge, index) => {
                const property = `border_${edge}` as const,
                  border = style[property];
                return (
                  <label key={edge}>
                    Граница {['сверху', 'справа', 'снизу', 'слева'][index]}
                    <span className="style-border-controls">
                      <select
                        aria-label={`Граница ${['сверху', 'справа', 'снизу', 'слева'][index]}`}
                        value={border?.line ?? ''}
                        disabled={disabled}
                        onChange={(event) =>
                          send(
                            event.target.value
                              ? {
                                  property,
                                  value: {
                                    line: event.target.value as Border['line'],
                                    color: border?.color ?? [0, 0, 0],
                                  },
                                }
                              : { property, reset: true },
                          )
                        }
                      >
                        <option value="">Нет</option>
                        {(['thin', 'medium', 'thick', 'dashed', 'dotted', 'double'] as const).map(
                          (line, i) => (
                            <option key={line} value={line}>
                              {
                                [
                                  'Тонкая',
                                  'Средняя',
                                  'Толстая',
                                  'Штриховая',
                                  'Пунктирная',
                                  'Двойная',
                                ][i]
                              }
                            </option>
                          ),
                        )}
                      </select>
                      <input
                        type="color"
                        aria-label={`Цвет границы ${['сверху', 'справа', 'снизу', 'слева'][index]}`}
                        disabled={disabled}
                        value={colorCss(border?.color ?? [0, 0, 0])}
                        onChange={(event) =>
                          send({
                            property,
                            value: {
                              line: border?.line ?? 'thin',
                              color: color(event.target.value),
                            },
                          })
                        }
                      />
                    </span>
                  </label>
                );
              })}
            </div>
            {document.error && <p role="alert">{document.error}</p>}
            <p className="style-format-note">
              Даты: календарь 1900, диапазон 1–2958465; неподдерживаемое значение отображается как
              #FORMAT!. Высота строк пока фиксирована.
            </p>
            <button
              type="button"
              className="style-reset"
              disabled={disabled}
              onClick={() =>
                document.setStyle(
                  (Object.keys(DEFAULT_STYLE) as (keyof CellStyle)[]).map((property) => ({
                    property,
                    reset: true,
                  })),
                )
              }
            >
              Сбросить оформление
            </button>
          </>
        )}
      </dialog>
    </>
  );
}
