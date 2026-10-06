import { useSyncExternalStore } from 'react';

export type ScrollbarMode = 'smart' | 'ordinary';
const KEY = 'pxel.scrollbar.v1';

export class ScrollbarPreference {
  private mode: ScrollbarMode = 'smart';
  private listeners = new Set<() => void>();
  constructor(private storage?: Pick<Storage, 'getItem' | 'setItem'>) {
    try {
      const value = storage?.getItem(KEY);
      if (value === 'smart' || value === 'ordinary') this.mode = value;
    } catch {
      // Unavailable storage keeps the default; changes still work for this session.
    }
  }
  snapshot = () => this.mode;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
  set(mode: ScrollbarMode) {
    if (mode === this.mode) return;
    this.mode = mode;
    try {
      this.storage?.setItem(KEY, mode);
    } catch {
      // Keep the current session choice when persistence is blocked.
    }
    this.listeners.forEach((listener) => listener());
  }
}

let storage: Storage | undefined;
try {
  storage = globalThis.localStorage;
} catch {
  // Browser privacy settings can deny access even to the storage getter.
}
export const scrollbarPreference = new ScrollbarPreference(storage);
export const useScrollbarMode = () =>
  useSyncExternalStore(scrollbarPreference.subscribe, scrollbarPreference.snapshot);
