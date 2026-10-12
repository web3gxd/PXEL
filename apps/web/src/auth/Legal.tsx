const configuredSource = import.meta.env.VITE_PXEL_SOURCE_URL as string | undefined;
const source =
  configuredSource &&
  (configuredSource.startsWith('https://') || /^\/(?!\/)/.test(configuredSource))
    ? configuredSource
    : 'https://github.com/web3gxd/PXEL';

export function Legal() {
  return (
    <p className="auth-legal">
      PXEL © 2026 <a href="https://github.com/web3gxd">web3gxd</a>.{' '}
      <a href="/license.txt">AGPL-3.0</a> · <a href={source}>Исходный код</a>. Без гарантий.
      Распространение — на условиях лицензии.{' '}
      <a href="/third-party-notices.txt">Сторонние компоненты</a>.
    </p>
  );
}
