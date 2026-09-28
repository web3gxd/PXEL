import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { chromium, expect } from '@playwright/test';

const deck = new URL('../../../presentation/index.html', import.meta.url).href;
const screenshotDir = new URL('../../../.local/presentation/', import.meta.url);
const browser = await chromium.launch();

try {
  for (const viewport of [
    { width: 1440, height: 900 },
    { width: 390, height: 844 },
  ]) {
    const page = await browser.newPage({ viewport });
    const errors = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.goto(deck);
    assert.equal(await page.locator('.slide').count(), 10);
    assert.equal(await page.locator('#counter').textContent(), '01 / 10');
    if (process.env.PXEL_PRESENTATION_SCREENSHOTS === '1') {
      await mkdir(screenshotDir, { recursive: true });
      await page.screenshot({
        path: fileURLToPath(new URL(`cover-${viewport.width}.png`, screenshotDir)),
      });
    }
    await page.getByRole('button', { name: 'Следующий слайд' }).click();
    assert.equal(await page.locator('#counter').textContent(), '02 / 10');
    await page.keyboard.press('End');
    assert.equal(await page.locator('#counter').textContent(), '10 / 10');
    await page.keyboard.press('Home');
    assert.equal(await page.locator('#counter').textContent(), '01 / 10');
    await page.goto(`${deck}#s06`);
    await expect(page.locator('#counter')).toHaveText('06 / 10');
    if (process.env.PXEL_PRESENTATION_SCREENSHOTS === '1') {
      await page.screenshot({
        path: fileURLToPath(new URL(`metrics-${viewport.width}.png`, screenshotDir)),
      });
      await page.goto(`${deck}#s09`);
      await expect(page.locator('#counter')).toHaveText('09 / 10');
      await page.screenshot({
        path: fileURLToPath(new URL(`roadmap-${viewport.width}.png`, screenshotDir)),
      });
    }
    const horizontalOverflow = await page.evaluate(
      () => document.documentElement.scrollWidth > innerWidth + 1,
    );
    assert.equal(horizontalOverflow, false, `horizontal overflow at ${viewport.width}px`);
    assert.deepEqual(errors, []);
    await page.close();
    process.stdout.write(
      `Presentation ${viewport.width}x${viewport.height}: 10 slides, navigation, layout OK\n`,
    );
  }
} finally {
  await browser.close();
}
