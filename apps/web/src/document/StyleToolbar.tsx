import { useEffect, useRef, useState } from 'react';
import {
  PaintBucket,
  TextAa,
  TextAlignCenter,
  TextAlignJustify,
  TextAlignLeft,
  TextAlignRight,
  TextB,
  TextItalic,
  TextStrikethrough,
  TextUnderline,
  X,
} from '@phosphor-icons/react';
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
  const mac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);
  const mod = mac ? '⌘' : 'Ctrl+';
  const send = (change: StyleChange) => document.setStyle([change]);
  const toggle = (property: 'font_bold' | 'font_italic' | 'font_strike') =>
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
          aria-pressed={
            document.selectionStyle('font_bold') === 'mixed' ? 'mixed' : style.font_bold
          }
          title={`Полужирный (${mod}B)`}
          disabled={disabled}
          onClick={() => toggle('font_bold')}
        >
          <TextB size={18} weight="bold" aria-hidden="true" />
        </button>
        <button
          type="button"
          className="tool"
          aria-label="Курсив"
          aria-pressed={
            document.selectionStyle('font_italic') === 'mixed' ? 'mixed' : style.font_italic
          }
          title={`Курсив (${mod}I)`}
          disabled={disabled}
          onClick={() => toggle('font_italic')}
        >
          <TextItalic size={18} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="tool"
          aria-label="Подчёркивание"
          aria-pressed={
            document.selectionStyle('font_underline') === 'mixed'
              ? 'mixed'
              : style.font_underline !== 'none'
          }
          title={`Подчёркивание (${mod}U)`}
          disabled={disabled}
          onClick={() =>
            send({
              property: 'font_underline',
              value: style.font_underline === 'single' ? 'none' : 'single',
            })
          }
        >
          <TextUnderline size={18} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="tool"
          aria-label="Зачёркивание"
          aria-pressed={
            document.selectionStyle('font_strike') === 'mixed' ? 'mixed' : style.font_strike
          }
          title={mac ? 'Зачёркивание (⌘⇧X)' : 'Зачёркивание (Alt+Shift+5)'}
          disabled={disabled}
          onClick={() => toggle('font_strike')}
        >
          <TextStrikethrough size={18} aria-hidden="true" />
        </button>
        <label className="style-overflow" title="Режим текста">
          <TextAlignJustify size={18} aria-hidden="true" />
          <select
            className="style-select"
            aria-label="Режим текста"
            value={
              document.selectionStyle('text_overflow') === 'mixed' ? 'mixed' : style.text_overflow
            }
            disabled={disabled}
            onChange={(event) =>
              event.target.value !== 'mixed' &&
              send({
                property: 'text_overflow',
                value: event.target.value as CellStyle['text_overflow'],
              })
            }
          >
            <option value="mixed">Смешанный</option>
            <option value="clip">Обрезать</option>
            <option value="wrap">Переносить</option>
            <option value="overflow">Выходить в пустые ячейки</option>
          </select>
        </label>
        <label className="style-color" title="Цвет текста">
          <TextAa size={18} aria-hidden="true" />
          <input
            type="color"
            aria-label="Цвет текста"
            value={colorCss(style.font_color)}
            disabled={disabled}
            onChange={(event) => send({ property: 'font_color', value: color(event.target.value) })}
          />
        </label>
        <label className="style-color" title="Цвет заливки">
          <PaintBucket size={18} aria-hidden="true" />
          <input
            type="color"
            aria-label="Цвет заливки"
            value={colorCss(style.fill)}
            disabled={disabled}
            onChange={(event) => send({ property: 'fill', value: color(event.target.value) })}
          />
        </label>
        {(
          [
            ['left', TextAlignLeft],
            ['center', TextAlignCenter],
            ['right', TextAlignRight],
          ] as const
        ).map(([alignment, AlignmentIcon]) => (
          <button
            key={alignment}
            type="button"
            className="tool"
            aria-label={`Выравнивание: ${alignment === 'left' ? 'слева' : alignment === 'center' ? 'по центру' : 'справа'}`}
            aria-pressed={
              document.selectionStyle('horizontal') === 'mixed'
                ? 'mixed'
                : style.horizontal === alignment
            }
            title={`Выравнивание ${alignment === 'left' ? 'слева' : alignment === 'center' ? 'по центру' : 'справа'} (${mod}Shift+${alignment === 'left' ? 'L' : alignment === 'center' ? 'E' : 'R'})`}
            disabled={disabled}
            onClick={() => send({ property: 'horizontal', value: alignment })}
          >
            <AlignmentIcon size={18} aria-hidden="true" />
          </button>
        ))}
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
                className="tool sharing-close"
                aria-label="Закрыть оформление"
                onClick={() => dialog.current?.close()}
              >
                <X size={18} aria-hidden="true" />
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
              <label>
                Режим отображения текста
                <select
                  value={
                    document.selectionStyle('text_overflow') === 'mixed'
                      ? 'mixed'
                      : style.text_overflow
                  }
                  disabled={disabled}
                  onChange={(event) =>
                    event.target.value !== 'mixed' &&
                    send({
                      property: 'text_overflow',
                      value: event.target.value as CellStyle['text_overflow'],
                    })
                  }
                >
                  <option value="mixed">Смешанный</option>
                  <option value="clip">Обрезать</option>
                  <option value="wrap">Переносить</option>
                  <option value="overflow">Выходить в пустые ячейки</option>
                </select>
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
