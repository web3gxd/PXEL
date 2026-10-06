import { expect, type Page } from '@playwright/test';

/** Durable authorization makes clipboard writes asynchronous; wait before moving the selection. */
export async function copySelection(page: Page) {
  await writeSelection(page, 'C');
}

export async function cutSelection(page: Page) {
  await writeSelection(page, 'X');
}

async function writeSelection(page: Page, key: 'C' | 'X') {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  const sentinel = `PXEL clipboard pending ${Date.now()}`;
  await page.evaluate((text) => navigator.clipboard.writeText(text), sentinel);
  await page.keyboard.press(`ControlOrMeta+${key}`);
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).not.toBe(sentinel);
}

/** The Windows system clipboard converts LF to CRLF; internal PXEL data retains LF. */
export async function readClipboardText(page: Page) {
  return (await page.evaluate(() => navigator.clipboard.readText())).replaceAll('\r\n', '\n');
}
