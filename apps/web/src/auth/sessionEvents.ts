const AUTH_CHANGED = 'pxel-auth-changed';

export function refreshAccount(): void {
  window.dispatchEvent(new Event(AUTH_CHANGED));
}

export function onAccountChanged(listener: () => void): () => void {
  window.addEventListener(AUTH_CHANGED, listener);
  return () => window.removeEventListener(AUTH_CHANGED, listener);
}
