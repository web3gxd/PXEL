import { expect, type Locator } from '@playwright/test';

/** Exercise the actual custom popup, including its disabled choices. */
export async function selectValue(trigger: Locator, value: string) {
  // The listbox has the same accessible name; legacy getByLabel callers may match both.
  trigger = trigger.and(trigger.page().getByRole('combobox'));
  await expect(trigger).toBeEnabled();
  await trigger.click();
  const id = await trigger.getAttribute('aria-controls');
  if (!id) throw new Error('Missing custom select listbox');
  const list = trigger.page().locator(`[id=${JSON.stringify(id)}]`);
  await expect(list).toBeVisible();
  const option = list.locator(`[role="option"][data-value=${JSON.stringify(value)}]`);
  await expect(option).not.toHaveAttribute('aria-disabled', 'true');
  await option.click();
  await expect(trigger).toHaveAttribute('aria-expanded', 'false');
}
