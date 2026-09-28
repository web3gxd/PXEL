import { expect, test } from '@playwright/test';

test('real binary handshake, viewport Canvas and keyboard scroll', async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Новая таблица' })).toBeVisible();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено', {
    timeout: 15_000,
  });
  const canvas = page.locator('canvas');
  await expect(canvas).toHaveAttribute('data-ready', 'true');
  const measurements = await canvas.evaluate((element: HTMLCanvasElement) => ({
    width: element.width,
    height: element.height,
    cssWidth: element.getBoundingClientRect().width,
    cssHeight: element.getBoundingClientRect().height,
    dpr: devicePixelRatio,
    domNodes: document.querySelectorAll('*').length,
    gridReadyMs: performance.getEntriesByName('pxel:grid-ready')[0]?.startTime,
    userAgent: navigator.userAgent,
    viewport: { width: innerWidth, height: innerHeight },
    headerPixel: Array.from(element.getContext('2d')?.getImageData(10, 10, 1, 1).data ?? []),
    cellPixel: Array.from(
      element.getContext('2d')?.getImageData(60 * devicePixelRatio, 40 * devicePixelRatio, 1, 1)
        .data ?? [],
    ),
  }));
  expect(measurements.width).toBe(Math.round(measurements.cssWidth * measurements.dpr));
  expect(measurements.height).toBe(Math.round(measurements.cssHeight * measurements.dpr));
  expect(measurements.cssWidth).toBeGreaterThan(300);
  expect(measurements.cssHeight).toBeGreaterThan(200);
  expect(measurements.domNodes).toBeLessThan(250);
  expect(measurements.gridReadyMs).toBeDefined();
  expect(measurements.headerPixel).not.toEqual([255, 255, 255, 255]);
  expect(measurements.cellPixel).toEqual([255, 255, 255, 255]);
  await testInfo.attach('grid-observation', {
    body: JSON.stringify(measurements, null, 2),
    contentType: 'application/json',
  });
  const viewport = page.getByRole('region', { name: 'Область таблицы' });
  await page.getByRole('button', { name: 'Создать таблицу' }).focus();
  await page.keyboard.press('Tab');
  await expect(viewport).toBeFocused();
  await page.keyboard.press('PageDown');
  await expect.poll(() => viewport.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  await expect(canvas).toHaveCount(1);
  expect(await page.locator('*').count()).toBe(measurements.domNodes);
  await page.setViewportSize({ width: 960, height: 700 });
  await expect
    .poll(() => canvas.evaluate((element: HTMLCanvasElement) => element.width))
    .toBeLessThan(measurements.width);
  const resized = await canvas.evaluate((element: HTMLCanvasElement) => ({
    backing: element.width,
    expected: Math.round(element.getBoundingClientRect().width * devicePixelRatio),
  }));
  expect(resized.backing).toBe(resized.expected);
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено', {
    timeout: 15_000,
  });
  expect(errors).toEqual([]);
});

test('compact layout retains the viewport without horizontal page overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await expect(page.locator('canvas')).toHaveAttribute('data-ready', 'true');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено', {
    timeout: 15_000,
  });
  const size = await page.evaluate(() => ({
    content: document.documentElement.scrollWidth,
    viewport: innerWidth,
  }));
  expect(size.content).toBe(size.viewport);
  await expect(page.getByRole('region', { name: 'Область таблицы' })).toBeVisible();
});
