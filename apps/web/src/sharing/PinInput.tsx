import { useRef } from 'react';
import { changePin } from './policy';
export function PinInput({
  value,
  onChange,
  disabled = false,
}: {
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
}) {
  const refs = useRef<(HTMLInputElement | null)[]>([]);
  const update = (index: number, text: string) => {
    const changed = changePin(value, index, text);
    onChange(changed.pin);
    refs.current[changed.focus]?.focus();
  };
  return (
    <div className="sharing-pin" role="group" aria-label="PIN из шести цифр">
      {Array.from({ length: 6 }, (_, index) => (
        <input
          key={index}
          ref={(element) => {
            refs.current[index] = element;
          }}
          type="text"
          inputMode="numeric"
          autoComplete={index === 0 ? 'one-time-code' : 'off'}
          maxLength={6}
          disabled={disabled}
          aria-label={`Цифра PIN ${index + 1}`}
          value={value[index] === ' ' ? '' : (value[index] ?? '')}
          onChange={(event) => update(index, event.target.value)}
          onFocus={(event) => event.currentTarget.select()}
          onPaste={(event) => {
            event.preventDefault();
            update(index, event.clipboardData.getData('text/plain'));
          }}
          onKeyDown={(event) => {
            if (event.key === 'Backspace') {
              event.preventDefault();
              const target = value[index]?.trim() ? index : Math.max(0, index - 1);
              onChange(changePin(value, target, '').pin);
              refs.current[target]?.focus();
            } else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
              event.preventDefault();
              refs.current[
                Math.max(0, Math.min(5, index + (event.key === 'ArrowLeft' ? -1 : 1)))
              ]?.focus();
            }
          }}
        />
      ))}
    </div>
  );
}
