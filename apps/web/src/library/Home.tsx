import { useCallback, useEffect, useRef, useState } from 'react';
import {
  BookOpenIcon,
  StarIcon,
  PlusIcon,
  SquaresFourIcon,
  ListIcon,
  ArrowUpRightIcon,
  FileArrowUpIcon,
  MagnifyingGlassIcon,
} from '@phosphor-icons/react';
import {
  createBook,
  favorite,
  readPage,
  setLayout,
  visible,
  copyTitle,
  mergeEntries,
  type Entry,
  type Layout,
} from './api';
import { PxelPanel } from '../document/PxelPanel';
import { parseMarkdownInWorker } from '../clipboard/markdown-worker-client';
import { uuidV7 } from '../document/store';
import './library.css';
const templates = [
  {
    id: 'budget',
    title: 'Бюджет',
    description: 'План, расходы и остаток',
    detail: 'Суммы · остаток · итоги',
    icon: '₽',
  },
  {
    id: 'tasks',
    title: 'Учёт задач',
    description: 'Статусы, команда и время',
    detail: 'Готовые задачи · часы',
    icon: '✓',
  },
  {
    id: 'sales',
    title: 'План продаж',
    description: 'План и продажи по месяцам',
    detail: 'Выручка · выполнение, %',
    icon: '↗',
  },
] as const;
export function Home({ onOpen }: { onOpen: (id: string, markdown?: string) => void }) {
  const [entries, setEntries] = useState<Entry[]>([]),
    [layout, setCurrentLayout] = useState<Layout>({
      schema_version: 1,
      revision: 0,
      layout: 'cards',
    }),
    [query, setQuery] = useState(''),
    [section, setSection] = useState<'all' | 'recent' | 'favorites' | 'shared'>('all'),
    [loading, setLoading] = useState(true),
    [ready, setReady] = useState(false),
    [busy, setBusy] = useState(''),
    [error, setError] = useState(''),
    [pxel, setPxel] = useState(false),
    [cursor, setCursor] = useState<string | null>(null);
  const file = useRef<HTMLInputElement>(null),
    generation = useRef(0),
    creation = useRef<{ template: string; document_id: string; request_id: string } | undefined>(
      undefined,
    );
  const load = useCallback(async (signal: AbortSignal, initial: string | null = null) => {
    const run = ++generation.current;
    setLoading(true);
    setError('');
    try {
      const session = await fetch('/api/dev/session', { method: 'POST', signal });
      if (!session.ok) throw new Error('Требуется авторизация для открытия библиотеки.');
      if (signal.aborted || run !== generation.current) return;
      setReady(true);
      let cursor = initial;
      const found: Entry[] = [];
      let page;
      do {
        page = await readPage(cursor, signal);
        found.push(...page.documents);
        cursor = page.next_cursor;
        if (signal.aborted || run !== generation.current) return;
        setEntries((previous) => mergeEntries(previous, found, initial !== null));
        const incomingLayout = page.layout;
        setCurrentLayout((previous) =>
          previous.revision > incomingLayout.revision ? previous : incomingLayout,
        );
      } while (cursor && found.length < 10000 && !signal.aborted);
      if (signal.aborted || run !== generation.current) return;
      setCursor(cursor);
    } catch (e) {
      if (!signal.aborted && run === generation.current)
        setError(e instanceof Error ? e.message : 'Не удалось загрузить библиотеку.');
    } finally {
      if (!signal.aborted && run === generation.current) setLoading(false);
    }
  }, []);
  useEffect(() => {
    const abort = new AbortController();
    void load(abort.signal);
    return () => abort.abort();
  }, [load]);
  const create = async (template?: (typeof templates)[number]['id'], markdown?: string) => {
    setBusy(template ?? 'blank');
    setError('');
    try {
      if (!creation.current || creation.current.template !== (template ?? 'blank'))
        creation.current = {
          template: template ?? 'blank',
          document_id: uuidV7(),
          request_id: uuidV7(),
        };
      const id = await createBook(template, creation.current);
      creation.current = undefined;
      onOpen(id, markdown);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Не удалось создать книгу.');
    } finally {
      setBusy('');
    }
  };
  const toggle = async (entry: Entry) => {
    setBusy(entry.document_id);
    setError('');
    try {
      const preference = await favorite(entry, !entry.preference.favorite);
      setEntries((all) =>
        all.map((e) => (e.document_id === entry.document_id ? { ...e, preference } : e)),
      );
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Не удалось сохранить избранное.');
    } finally {
      setBusy('');
    }
  };
  const changeLayout = async (value: Layout['layout']) => {
    setBusy('layout');
    try {
      setCurrentLayout(await setLayout(layout, value));
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Не удалось сохранить вид.');
    } finally {
      setBusy('');
    }
  };
  const books = visible(entries, query, section);
  const [shown, setShown] = useState(100);
  useEffect(() => setShown(100), [query, section]);
  return (
    <main
      className="library"
      onCopy={(event) => {
        const selection = window.getSelection();
        if (!selection || !selection.rangeCount) return;
        const range = selection.getRangeAt(0);
        const card = (node: Node) =>
          (node instanceof Element ? node : node.parentElement)?.closest<HTMLElement>(
            '[data-book-id]',
          );
        const first = card(range.startContainer),
          last = card(range.endContainer);
        const intersectsBook = Array.from(
          event.currentTarget.querySelectorAll('[data-book-id]'),
        ).some((element) => range.intersectsNode(element));
        if (!intersectsBook) return;
        event.preventDefault();
        if (!first || first !== last) {
          setError('Копируйте текст одной книги за один раз.');
          return;
        }
        const entry = entries.find((e) => e.document_id === first.dataset.bookId);
        if (entry)
          void copyTitle(entry, selection.toString()).catch((e) =>
            setError(e instanceof Error ? e.message : 'Копирование не разрешено.'),
          );
      }}
      onCut={(event) => {
        const selection = window.getSelection();
        if (!selection?.rangeCount) return;
        const range = selection.getRangeAt(0);
        if (
          Array.from(event.currentTarget.querySelectorAll('[data-book-id]')).some((element) =>
            range.intersectsNode(element),
          )
        ) {
          event.preventDefault();
          setError('Название в библиотеке доступно только для копирования.');
        }
      }}
    >
      <header className="library-header">
        <a className="library-wordmark" href="/" aria-label="PXEL — библиотека">
          <span className="library-logo">
            <BookOpenIcon size={24} />
          </span>
          PXEL
        </a>
        <details
          className="library-storage library-encryption"
          onKeyDown={(event) => {
            if (event.key === 'Escape') event.currentTarget.open = false;
          }}
        >
          <summary>Зашифровано при хранении</summary>
          <p>
            Названия, настройки, события книги, значения, формулы, данные отмены, снимки и
            комментарии защищены отдельным ключом книги. Сервер читает и расшифровывает их для
            работы и расчёта формул.
          </p>
        </details>
      </header>
      <div className="library-content">
        <div className="library-intro">
          <div>
            <p className="library-eyebrow">ВАШЕ РАБОЧЕЕ МЕСТО</p>
            <h1>Рабочие книги</h1>
            <p>Данные, расчёты и планы — в одном месте.</p>
          </div>
          <div className="library-actions">
            <button
              className="library-primary"
              disabled={!ready || !!busy}
              onClick={() => void create()}
            >
              <PlusIcon size={18} />
              Создать таблицу
            </button>
            <button disabled={!ready || !!busy} onClick={() => setPxel(true)}>
              <FileArrowUpIcon size={18} />
              Импорт .pxel
            </button>
            <button disabled={!ready || !!busy} onClick={() => file.current?.click()}>
              Импорт Markdown
            </button>
          </div>
        </div>
        <input
          ref={file}
          className="library-file"
          type="file"
          accept=".md,.markdown,.txt"
          aria-label="Markdown-файл для новой книги"
          onChange={(e) => {
            const selected = e.target.files?.[0];
            if (!selected) return;
            e.target.value = '';
            if (selected.size > 4 * 1024 * 1024) {
              setError('Markdown-файл должен быть не больше 4 МиБ.');
              return;
            }
            setBusy('markdown');
            void selected
              .text()
              .then(async (text) => {
                await parseMarkdownInWorker(text);
                await create(undefined, text);
              })
              .catch((e) =>
                setError(e instanceof Error ? e.message : 'Не удалось прочитать Markdown.'),
              )
              .finally(() => setBusy(''));
          }}
        />
        <section className="library-templates" aria-label="Рабочие шаблоны">
          <div className="library-section-heading">
            <h2>Начните с готового</h2>
            <span>Формулы и оформление уже настроены</span>
          </div>
          <div className="template-grid">
            {templates.map((t) => (
              <button
                key={t.id}
                disabled={!ready || !!busy}
                className={'template-card template-' + t.id}
                aria-label={'Создать из шаблона: ' + t.title}
                onClick={() => void create(t.id)}
              >
                <span className="template-symbol" aria-hidden="true">
                  {t.icon}
                </span>
                <div>
                  <h3>{t.title}</h3>
                  <p>{t.description}</p>
                  <small>{t.detail}</small>
                </div>
                <ArrowUpRightIcon className="template-arrow" size={20} />
              </button>
            ))}
          </div>
        </section>
        <section aria-label="Библиотека книг">
          <div className="library-toolbar">
            <div className="library-tabs" aria-label="Разделы библиотеки">
              {(
                [
                  ['all', 'Все книги'],
                  ['recent', 'Недавние'],
                  ['favorites', 'Избранные'],
                  ['shared', 'Доступные мне'],
                ] as const
              ).map(([value, title]) => (
                <button
                  key={value}
                  aria-pressed={section === value}
                  onClick={() => setSection(value)}
                >
                  {title}
                </button>
              ))}
            </div>
            <div className="library-search">
              <MagnifyingGlassIcon size={18} aria-hidden="true" />
              <input
                type="search"
                aria-label="Поиск по названию"
                placeholder="Найти книгу…"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
            </div>
            <div className="library-layout" aria-label="Вид библиотеки">
              <button
                aria-label="Карточки"
                aria-pressed={layout.layout === 'cards'}
                disabled={!!busy}
                onClick={() => void changeLayout('cards')}
              >
                <SquaresFourIcon size={19} />
              </button>
              <button
                aria-label="Список"
                aria-pressed={layout.layout === 'list'}
                disabled={!!busy}
                onClick={() => void changeLayout('list')}
              >
                <ListIcon size={19} />
              </button>
            </div>
          </div>
          {loading && (
            <p className="library-state" role="status">
              Загружаем библиотеку…
            </p>
          )}
          {error && (
            <div className="library-state" role="alert">
              <p>{error}</p>
              <button onClick={() => void load(new AbortController().signal)}>
                Обновить библиотеку
              </button>
            </div>
          )}
          {!loading && !error && books.length === 0 && (
            <div className="library-empty">
              <BookOpenIcon size={44} aria-hidden="true" />
              <h3>{query ? 'Книги не найдены' : 'Здесь будут ваши книги'}</h3>
              <p>
                {query
                  ? 'Попробуйте другое название.'
                  : 'Создайте первую таблицу, выберите шаблон или импортируйте файл.'}
              </p>
            </div>
          )}
          <div className={'book-collection ' + layout.layout}>
            {books.slice(0, shown).map((entry) => (
              <article
                key={entry.document_id}
                className="book-card"
                data-book-id={entry.document_id}
              >
                <div className="book-cover" aria-hidden="true">
                  <BookOpenIcon size={30} />
                  <span />
                </div>
                <div className="book-details">
                  <a className="book-title" href={'/?document=' + entry.document_id}>
                    {entry.metadata.title}
                  </a>
                  <p>
                    {entry.role === 'owner'
                      ? 'Моя книга'
                      : entry.role === 'editor'
                        ? 'Можно редактировать'
                        : 'Только просмотр'}
                    <span> · </span>
                    {new Intl.DateTimeFormat('ru', { dateStyle: 'medium' }).format(
                      entry.preference.opened_at || entry.created_at,
                    )}
                  </p>
                </div>
                <button
                  className="book-star"
                  aria-label={
                    (entry.preference.favorite ? 'Убрать из избранного: ' : 'В избранное: ') +
                    entry.metadata.title
                  }
                  aria-pressed={entry.preference.favorite}
                  disabled={!!busy}
                  onClick={() => void toggle(entry)}
                >
                  <StarIcon size={20} weight={entry.preference.favorite ? 'fill' : 'regular'} />
                </button>
              </article>
            ))}
          </div>
          {!loading && books.length > shown && (
            <button className="library-more" onClick={() => setShown((n) => n + 100)}>
              Показать ещё книги
            </button>
          )}
          {cursor && !loading && (
            <button
              className="library-more"
              onClick={() => void load(new AbortController().signal, cursor)}
            >
              Загрузить следующие книги
            </button>
          )}
        </section>
        <footer className="library-footer">
          <span>PXEL</span>
          <p>
            Книги доступны согласно вашим текущим правам. Пароли файлов и защищённых ссылок
            передавайте отдельно.
          </p>
        </footer>
      </div>
      {pxel && <PxelPanel onClose={() => setPxel(false)} onImported={(id) => onOpen(id)} />}
    </main>
  );
}
