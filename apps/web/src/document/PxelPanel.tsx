import { Select, SelectOption } from '../ui/Select';
import { useState } from 'react';
import { Modal } from '../sharing/Modal';
import type { DocumentStore } from './store';
import {
  fileError,
  importFile,
  previewFile,
  readFile,
  validPassword,
  type Composition,
  type ImportPayload,
} from './pxel-file';

export function PxelPanel({
  document,
  onClose,
  onImported,
}: {
  document?: DocumentStore;
  onClose: () => void;
  onImported?: (id: string) => void;
}) {
  const [mode, setMode] = useState<'export' | 'import'>(document ? 'export' : 'import'),
    [password, setPassword] = useState(''),
    [confirm, setConfirm] = useState(''),
    [anonymous, setAnonymous] = useState(false),
    [file, setFile] = useState<Omit<ImportPayload, 'password'>>(),
    [composition, setComposition] = useState<Composition>(),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(''),
    [status, setStatus] = useState('');
  const run = async () => {
    setBusy(true);
    setError('');
    setStatus('');
    try {
      if (mode === 'export') {
        const head = document?.projection;
        if (!head || !document || document.pending.length || !head.capabilities?.can_export_pxel)
          throw new Error('Дождитесь сохранения; экспорт должен быть разрешён владельцем.');
        const response = await fetch(document.documentUrl(head.document_id, '/export.pxel'), {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            schema_version: 1,
            password,
            document_version: head.version,
            anonymized: anonymous || !!document.privacyMask,
          }),
          signal: AbortSignal.timeout(60_000),
        });
        if (!response.ok) throw await fileError(response);
        const url = URL.createObjectURL(await response.blob());
        const link = window.document.createElement('a');
        link.href = url;
        link.download = 'PXEL.pxel';
        link.click();
        setTimeout(() => URL.revokeObjectURL(url), 30_000);
        setPassword('');
        setConfirm('');
        setStatus('Зашифрованный файл скачан. Пароль передавайте отдельно.');
      } else {
        if (!file) throw new Error('Выберите файл.');
        setComposition(await previewFile({ ...file, password }));
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Не удалось обработать файл.');
    } finally {
      setBusy(false);
    }
  };
  const publish = async () => {
    if (!file || !composition) return;
    setBusy(true);
    setError('');
    try {
      if (document && (document.pending.length || document.editing))
        throw new Error('Сохраните текущие изменения перед открытием импортированной книги.');
      const id = await importFile({ ...file, password });
      setPassword('');
      setComposition(undefined);
      if (onImported) onImported(id);
      else location.assign('/?document=' + encodeURIComponent(id));
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Не удалось импортировать файл.');
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      title="Файл PXEL"
      clipboard="credentials"
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      {document && !document.isGuest && (
        <label>
          Действие
          <Select
            aria-label="Действие"
            disabled={busy}
            value={mode}
            onValueChange={(e) => {
              setMode(e as typeof mode);
              setComposition(undefined);
              setError('');
              setPassword('');
            }}
          >
            <SelectOption value="export">Экспорт</SelectOption>
            <SelectOption value="import">Импорт новой книги</SelectOption>
          </Select>
        </label>
      )}
      <p>
        Файл содержит текущие листы, формулы, оформление и настройки. История, комментарии,
        пользователи и ссылки не переносятся.
      </p>
      {mode === 'import' && (
        <label>
          Файл .pxel
          <input
            type="file"
            accept=".pxel"
            disabled={busy}
            onChange={(e) => {
              setComposition(undefined);
              setFile(undefined);
              setError('');
              const selected = e.target.files?.[0];
              if (selected)
                void readFile(selected)
                  .then(setFile)
                  .catch((e) =>
                    setError(e instanceof Error ? e.message : 'Не удалось прочитать файл.'),
                  );
            }}
          />
        </label>
      )}
      <label>
        Пароль файла
        <input
          type="password"
          autoComplete="new-password"
          disabled={busy}
          value={password}
          onChange={(e) => {
            setPassword(e.target.value);
            setComposition(undefined);
          }}
        />
      </label>
      <p>12–128 символов. Пробелы и составные Unicode-символы сохраняются.</p>
      {mode === 'export' && (
        <>
          <label>
            Повторите пароль
            <input
              type="password"
              autoComplete="new-password"
              disabled={busy}
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
            />
          </label>
          <label>
            <input
              type="checkbox"
              checked={anonymous}
              disabled={busy}
              onChange={(e) => setAnonymous(e.target.checked)}
            />
            Обезличенная копия с рассчитанными значениями
          </label>
        </>
      )}
      <button
        type="button"
        disabled={
          busy ||
          !validPassword(password) ||
          (mode === 'export' &&
            (password !== confirm || !document?.projection?.capabilities?.can_export_pxel)) ||
          (mode === 'import' && !file)
        }
        onClick={() => void run()}
      >
        {busy ? 'Обрабатываем…' : mode === 'export' ? 'Скачать .pxel' : 'Проверить состав'}
      </button>
      {composition && (
        <section aria-label="Состав файла">
          <h3>{composition.title}</h3>
          <p>
            {composition.anonymized ? 'Обезличенная копия' : 'Текущая книга'} · Формул:{' '}
            {composition.formulas}
          </p>
          <ul>
            {composition.sheets.map((s, i) => (
              <li key={i}>
                {s.name}: {s.rows} × {s.columns}, ячеек {s.cells}
              </li>
            ))}
          </ul>
          <p>{composition.features.join(' · ')}</p>
          {composition.omitted_filters > 0 && (
            <p>
              Не перенесено фильтров: {composition.omitted_filters} (устаревшие границы или
              обезличенная копия).
            </p>
          )}
          <button type="button" disabled={busy} onClick={() => void publish()}>
            Создать новую книгу из файла
          </button>
        </section>
      )}
      {error && <p role="alert">{error}</p>}
      {status && <p role="status">{status}</p>}
    </Modal>
  );
}
