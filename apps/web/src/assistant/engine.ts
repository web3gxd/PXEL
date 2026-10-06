// Session-only action metadata. Contents are read on demand, never retained or sent.
export type Point = { row: number; column: number };
export type Observation = { kind: 'edit' | 'copy' | 'paste' | 'undo' | 'format'; position?: Point };
type Cell = { type: string; value?: unknown } | undefined;
export type Suggestion = {
  id: 'repeat' | 'series' | 'formula' | 'copy' | 'paste' | 'undo' | 'format';
  title: string;
  description: string;
  source?: { startRow: number; endRow: number; startColumn: number; endColumn: number };
  axis?: 'row' | 'column';
};

export class AssistantEngine {
  enabled = true;
  private actions: Observation[] = [];
  private dismissed = new Set<string>();
  setEnabled(enabled: boolean) {
    this.enabled = enabled;
    this.reset();
  }
  reset(clearDismissals = true) {
    this.actions = [];
    if (clearDismissals) this.dismissed.clear();
  }
  dismiss(id: string) {
    this.dismissed.add(id);
  }
  observe(action: Observation) {
    if (!this.enabled) return;
    this.actions.push({
      kind: action.kind,
      ...(action.position ? { position: { ...action.position } } : {}),
    });
    this.actions = this.actions.slice(-8);
  }
  suggest(read: (position: Point) => Cell): Suggestion | undefined {
    if (!this.enabled) return;
    const recent = this.actions.slice(-3);
    if (recent.length !== 3 || recent.some((a) => a.kind !== recent[0]?.kind)) return;
    const kind = recent[0]!.kind;
    let tip: Suggestion | undefined;
    if (kind !== 'edit') {
      const tips = {
        copy: [
          'Копирование без лишних кликов',
          'Выделите диапазон с Shift + стрелки, затем скопируйте его горячей клавишей.',
        ],
        paste: [
          'Заполните выделение одной вставкой',
          'Одно значение заполняет всё выделение. Блок повторяется, если размеры выделения кратны его размерам.',
        ],
        undo: [
          'Быстрая отмена и повтор',
          'Отменяйте правки и возвращайте отменённые действия с клавиатуры.',
        ],
        format: [
          'Оформите диапазон сразу',
          'Выделите нужные ячейки и примените оформление один раз. Для полужирного и курсива есть горячие клавиши.',
        ],
      };
      tip = { id: kind, title: tips[kind][0]!, description: tips[kind][1]! };
    } else {
      const positions = recent.map((a) => a.position);
      const [a, b, c] = positions;
      if (!a || !b || !c) return;
      const vertical =
        a.column === b.column &&
        b.column === c.column &&
        b.row === a.row + 1 &&
        c.row === b.row + 1;
      const horizontal =
        a.row === b.row &&
        b.row === c.row &&
        b.column === a.column + 1 &&
        c.column === b.column + 1;
      if (!vertical && !horizontal) return;
      const values = [read(a), read(b), read(c)];
      const source = { startRow: a.row, endRow: c.row, startColumn: a.column, endColumn: c.column };
      const axis = vertical ? 'row' : 'column';
      if (values.every((v) => v?.type === 'formula')) {
        tip = {
          id: 'formula',
          title: 'Формулы можно протянуть',
          description:
            'Выделите исходную формулу и потяните точку в углу: относительные ссылки сдвинутся автоматически. Для заполнения вниз используйте Ctrl+D / Cmd+D.',
        };
      } else if (values.every((v) => v?.type === 'number')) {
        const numbers = values.map((v) => Number(v?.value));
        const step = numbers[1]! - numbers[0]!;
        if (
          numbers.every(Number.isFinite) &&
          Number.isFinite(step) &&
          Math.abs(numbers[2]! - numbers[1]! - step) <= Math.max(1, Math.abs(step)) * 1e-10
        )
          tip = {
            id: step === 0 ? 'repeat' : 'series',
            title: step === 0 ? 'Повторяющийся ввод' : 'Обнаружена числовая последовательность',
            description:
              'Продолжите ряд в следующих пустых ячейках одним действием или протяните точку в углу выделения.',
            source,
            axis,
          };
      } else if (
        values[0]?.type !== 'blank' &&
        values[0] &&
        values.every((v) => v?.type === values[0]?.type && v?.value === values[0]?.value)
      ) {
        tip = {
          id: 'repeat',
          title: 'Повторяющийся ввод',
          description:
            'Заполните следующие пустые ячейки одним действием. Вниз также можно заполнить через Ctrl+D / Cmd+D.',
          source,
          axis,
        };
      }
    }
    return tip && !this.dismissed.has(tip.id) ? tip : undefined;
  }
}
