import { useId, useState } from 'react';
import type { EncryptionInfo } from './api';
import { formatExpiry } from './policy';
export function EncryptionBadge({ encryption }: { encryption: EncryptionInfo | undefined }) {
  const id = useId();
  const [visible, setVisible] = useState(false);
  if (!encryption) return null;
  return (
    <span className="sharing-encryption">
      <button
        className="sharing-lock"
        type="button"
        aria-label="Шифрование данных"
        aria-describedby={visible ? id : undefined}
        aria-expanded={visible}
        onClick={() => setVisible((previous) => !previous)}
        onMouseEnter={() => setVisible(true)}
        onMouseLeave={() => setVisible(false)}
        onFocus={() => setVisible(true)}
        onBlur={() => setVisible(false)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') setVisible(false);
        }}
      >
        <svg
          width="15"
          height="15"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.6"
          aria-hidden="true"
        >
          <rect x="5" y="10" width="14" height="11" rx="2" />
          <path d="M8 10V7a4 4 0 0 1 8 0v3M12 14v3" />
        </svg>
        <span>Зашифровано при хранении</span>
      </button>
      {visible && (
        <span id={id} role="tooltip" className="sharing-encryption-tooltip">
          События книги, значения, формулы, данные отмены, снимки и комментарии на диске защищены
          XChaCha20-Poly1305 отдельным ключом книги. Сервер читает и расшифровывает их для работы и
          расчёта формул.
          <span>
            {encryption.kek_rotated_at === null
              ? 'Ротация ключа ещё не выполнялась.'
              : `Ключ обёртки обновлён: ${formatExpiry(encryption.kek_rotated_at)}.`}
          </span>
        </span>
      )}
    </span>
  );
}
