import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { DocumentStore } from '../document/store';
import { Books, Plus } from '@phosphor-icons/react';
import { Modal } from '../sharing/Modal';
export function LibraryNavigation({
  document,
  online,
  children,
}: {
  document: DocumentStore;
  online: boolean;
  children?: (actions: { home: () => void; create: () => void; disabled: boolean }) => ReactNode;
}) {
  const [action, setAction] = useState<'home' | 'new'>(),
    [waiting, setWaiting] = useState(false),
    [error, setError] = useState('');
  const allowed = useRef(false),
    generation = useRef(0);
  useEffect(() => {
    const unload = (event: BeforeUnloadEvent) => {
      if (!allowed.current && (document.pending.length || document.editing)) {
        event.preventDefault();
        event.returnValue = '';
      }
    };
    window.addEventListener('beforeunload', unload);
    return () => {
      generation.current++;
      document.navigationGuardActive = false;
      window.removeEventListener('beforeunload', unload);
    };
  }, [document]);
  const go = (target: 'home' | 'new') => {
    allowed.current = true;
    document.navigationGuardActive = false;
    document.cancel();
    if (target === 'home') location.assign('/');
    else {
      void document.createDocument().finally(() => {
        allowed.current = false;
      });
    }
  };
  const ask = (target: 'home' | 'new') => {
    setError('');
    if (document.pending.length || document.editing) {
      document.navigationGuardActive = true;
      setAction(target);
    } else go(target);
  };
  const save = async () => {
    if (!action) return;
    const run = ++generation.current;
    setWaiting(true);
    setError('');
    document.commit();
    const deadline = Date.now() + 30000;
    while (
      (document.pending.length || document.editing) &&
      Date.now() < deadline &&
      generation.current === run
    )
      await new Promise((resolve) => setTimeout(resolve, 100));
    if (generation.current !== run) return;
    setWaiting(false);
    if (document.pending.length || document.editing) {
      setError('Изменения ещё не сохранены. Проверьте подключение и ввод.');
      return;
    }
    setAction(undefined);
    go(action);
  };
  const close = () => {
    generation.current++;
    document.navigationGuardActive = false;
    setWaiting(false);
    setAction(undefined);
  };
  return (
    <>
      {children ? (
        children({
          home: () => ask('home'),
          create: () => ask('new'),
          disabled: document.loading || !online,
        })
      ) : (
        <>
          <button
            type="button"
            className="insert-row-action library-home"
            aria-label="Библиотека"
            title="Библиотека"
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => ask('home')}
          >
            <Books size={18} aria-hidden="true" /> Библиотека
          </button>
          <button
            type="button"
            className="create-document"
            aria-label="Создать таблицу"
            title="Создать таблицу"
            disabled={document.loading || !online}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => ask('new')}
          >
            <Plus size={18} aria-hidden="true" /> Создать таблицу
          </button>
        </>
      )}
      {action && (
        <Modal title="Несохранённые изменения" onClose={close}>
          <p>В книге есть ввод или правки, которые сервер ещё не подтвердил.</p>
          <p>
            При уходе без ожидания уже отправленные операции могут сохраниться. Неподтверждённый
            ввод в браузере будет потерян.
          </p>
          <button type="button" disabled={waiting} onClick={() => void save()}>
            {waiting ? 'Ожидаем сохранения…' : 'Сохранить и перейти'}
          </button>
          <button
            type="button"
            disabled={waiting}
            onClick={() => {
              const target = action;
              close();
              go(target);
            }}
          >
            Уйти без ожидания
          </button>
          <button type="button" onClick={close}>
            Остаться в книге
          </button>
          {error && <p role="alert">{error}</p>}
        </Modal>
      )}
    </>
  );
}
