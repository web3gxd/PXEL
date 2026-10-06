import { expect, test } from '@playwright/test';
import { createBookFromUi } from './create-book';

test('axis resize paints before a delayed commit and rolls back a cancelled drag', async ({
  page,
}) => {
  await page.addInitScript(() => {
    const send = WebSocket.prototype.send;
    const held: { socket: WebSocket; data: Parameters<WebSocket['send']>[0] }[] = [];
    Object.assign(window, {
      resizeProbe: {
        hold: false,
        held,
        release: () => {
          for (const frame of held.splice(0)) send.call(frame.socket, frame.data);
        },
      },
    });
    WebSocket.prototype.send = function (data) {
      const probe = (window as unknown as { resizeProbe: { hold: boolean } }).resizeProbe;
      if (probe.hold) held.push({ socket: this, data });
      else send.call(this, data);
    };
  });
  await page.goto('/');
  await createBookFromUi(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const box = (await grid.boundingBox())!;
  const extent = page.locator('.grid-extent');
  const originalWidth = await extent.evaluate((el) => (el as HTMLElement).offsetWidth);
  const originalHeight = await extent.evaluate((el) => (el as HTMLElement).offsetHeight);
  await page.evaluate(() => {
    (window as unknown as { resizeProbe: { hold: boolean } }).resizeProbe.hold = true;
  });
  await page.mouse.move(box.x + 46 + 112, box.y + 14);
  await page.mouse.down();
  await page.mouse.move(box.x + 46 + 172, box.y + 14, { steps: 10 });
  await expect(page.locator('.grid-size-tooltip')).toContainText('172 px');
  await expect
    .poll(() => extent.evaluate((el) => (el as HTMLElement).offsetWidth))
    .toBe(originalWidth + 60);
  await page.mouse.up();
  // No ACK can arrive: local geometry must update while the confirmed version stays fixed.
  await expect
    .poll(() => extent.evaluate((el) => (el as HTMLElement).offsetWidth))
    .toBe(originalWidth + 60);
  await expect(page.getByTestId('save-status')).not.toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document');
  const confirmed = await (await page.request.get(`/api/documents/${id}`)).json();
  expect(confirmed.sheets[0].column_widths).toEqual([]);
  await grid.click({ position: { x: 46 + 172 + 20, y: 42 } });
  await expect(page.getByTestId('active-cell')).toContainText('B1:');
  await page.mouse.move(box.x + 20, box.y + 28 + 28);
  await page.mouse.down();
  await page.mouse.move(box.x + 20, box.y + 28 + 48);
  await page.mouse.up();
  await expect
    .poll(() => extent.evaluate((el) => (el as HTMLElement).offsetHeight))
    .toBe(originalHeight + 20);
  // Cancelling a new drag must restore the existing pending size and send no command.
  await page.mouse.move(box.x + 46 + 172, box.y + 14);
  await page.mouse.down();
  await page.mouse.move(box.x + 46 + 222, box.y + 14);
  await expect(page.locator('.grid-size-tooltip')).toContainText('222 px');
  await grid.evaluate((el) =>
    el.dispatchEvent(new PointerEvent('pointercancel', { bubbles: true, pointerId: 1 })),
  );
  await page.mouse.up();
  await expect(page.locator('.grid-size-tooltip')).toHaveCount(0);
  await expect
    .poll(() => extent.evaluate((el) => (el as HTMLElement).offsetWidth))
    .toBe(originalWidth + 60);
  await page.evaluate(() => {
    const probe = (window as unknown as { resizeProbe: { hold: boolean; release(): void } })
      .resizeProbe;
    probe.hold = false;
    probe.release();
  });
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.reload();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect
    .poll(() => extent.evaluate((el) => (el as HTMLElement).offsetWidth))
    .toBe(originalWidth + 60);
  await expect
    .poll(() => extent.evaluate((el) => (el as HTMLElement).offsetHeight))
    .toBe(originalHeight + 20);
  await page.screenshot({ path: test.info().outputPath('resize-verified.png') });
});

test('committed row and column sizes reach a second browser tab', async ({ page }) => {
  await page.goto('/');
  await createBookFromUi(page);
  const peer = await page.context().newPage();
  await peer.goto(page.url());
  await expect(peer.getByTestId('save-status')).toHaveText('Сохранено');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const box = (await grid.boundingBox())!;
  const peerExtent = peer.locator('.grid-extent');
  const original = await peerExtent.evaluate((el) => ({
    width: (el as HTMLElement).offsetWidth,
    height: (el as HTMLElement).offsetHeight,
  }));
  await page.mouse.move(box.x + 46 + 112, box.y + 14);
  await page.mouse.down();
  await page.mouse.move(box.x + 46 + 182, box.y + 14);
  await page.mouse.up();
  await expect
    .poll(() => peerExtent.evaluate((el) => (el as HTMLElement).offsetWidth))
    .toBe(original.width + 70);
  await page.mouse.move(box.x + 20, box.y + 28 + 28);
  await page.mouse.down();
  await page.mouse.move(box.x + 20, box.y + 28 + 58);
  await page.mouse.up();
  await expect
    .poll(() => peerExtent.evaluate((el) => (el as HTMLElement).offsetHeight))
    .toBe(original.height + 30);
  await peer.close();
});
