import { describe, expect, it, vi } from 'vitest';
import { ScrollbarPreference } from './preferences';

describe('scrollbar preference', () => {
  it('persists the explicit choice and notifies all views without redundant changes', () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => {
        values.set(key, value);
      },
    };
    const preference = new ScrollbarPreference(storage);
    const listener = vi.fn();
    const unsubscribe = preference.subscribe(listener);
    expect(preference.snapshot()).toBe('smart');
    preference.set('ordinary');
    preference.set('ordinary');
    expect(listener).toHaveBeenCalledTimes(1);
    expect(new ScrollbarPreference(storage).snapshot()).toBe('ordinary');
    unsubscribe();
    preference.set('smart');
    expect(listener).toHaveBeenCalledTimes(1);
  });
  it('ignores invalid saved modes and supports a session choice when storage fails', () => {
    expect(
      new ScrollbarPreference({ getItem: () => 'automatic', setItem: () => {} }).snapshot(),
    ).toBe('smart');
    const preference = new ScrollbarPreference({
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      },
    });
    preference.set('ordinary');
    expect(preference.snapshot()).toBe('ordinary');
  });
});
