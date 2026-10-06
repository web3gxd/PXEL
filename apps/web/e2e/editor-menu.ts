import type { Page } from '@playwright/test';

const groups: Record<string, string> = {
  Библиотека: 'Файл',
  'Импорт Markdown': 'Файл',
  'Файл PXEL': 'Файл',
  'Скачать XLSX': 'Файл',
  'Журнал и версии': 'Файл',
  'Вид листа': 'Вид',
  Комментарии: 'Вид',
  'Показать оригиналы': 'Вид',
  'Строка ↑': 'Ячейки',
  'Столбец ↑': 'Ячейки',
  'Строка −': 'Ячейки',
  'Столбец −': 'Ячейки',
  Размеры: 'Ячейки',
  Сортировка: 'Данные',
  Пересчитать: 'Данные',
  'Персональные данные': 'Данные',
  'Выдача и аудит': 'Данные',
  'Проверка констант': 'Инструменты',
  Профайлер: 'Инструменты',
  'Горячие клавиши': 'Инструменты',
};

export async function editorButton(page: Page, label: string) {
  const group = groups[label];
  if (!group) throw new Error(`Unknown editor action: ${label}`);
  const trigger = page.getByRole('button', { name: group, exact: true });
  if ((await trigger.getAttribute('aria-expanded')) !== 'true') await trigger.click();
  const cells: Record<string, [string, string]> = {
    'Строка ↑': ['Добавить', 'Вставить строки листа'],
    'Столбец ↑': ['Добавить', 'Вставить столбцы листа'],
    'Строка −': ['Удалить', 'Удалить строки листа'],
    'Столбец −': ['Удалить', 'Удалить столбцы листа'],
    Размеры: ['Форматировать', 'Высота строки…'],
  };
  const path = cells[label];
  if (path) {
    const submenu = page
      .locator('.editor-menu-popup')
      .getByRole('button', { name: path[0], exact: true });
    if ((await submenu.getAttribute('aria-expanded')) !== 'true') await submenu.click();
  }
  return page
    .locator('.editor-menu-popup')
    .getByRole('button', { name: path?.[1] ?? label, exact: true });
}

export function editorTrigger(page: Page, label: string) {
  const group = groups[label];
  if (!group) throw new Error(`Unknown editor action: ${label}`);
  return page.getByRole('button', { name: group, exact: true });
}
