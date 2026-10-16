import { expect, test, type Locator, type Page } from '@playwright/test';

// The row menus used from the keyboard: when the dialog a menu's item opened
// is closed with Escape, the focus is on that row's own menu button, not at the
// top of the page. Against e2e/seed.sql, as main.spec.ts.

const rows = (page: Page) => page.locator('tbody tr');

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('e2e')) {
      localStorage.clear();
      sessionStorage.setItem('e2e', '1');
    }
  });
});

/** Opens a far-down row's menu with Enter on its button, and chooses an item with Enter. */
const choose = async (page: Page, item: string): Promise<Locator> => {
  await expect(rows(page)).toHaveCount(30);
  const tr = rows(page).nth(24);
  const summary = tr.locator('details.row-actions summary');
  await summary.focus();
  await page.keyboard.press('Enter');
  await expect(tr.locator('details.row-actions')).toHaveJSProperty('open', true);
  await tr.getByRole('button', { name: item }).focus();
  await page.keyboard.press('Enter');
  return summary;
};

test('Escape from the edit dialog puts the focus back on the row\'s menu button', async ({ page }) => {
  await page.goto('/');
  const summary = await choose(page, 'Edit text');
  const dialog = page.getByRole('dialog', { name: 'Edit the text' });
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(summary).toBeFocused();
});

test('Escape from the share dialog puts the focus back on the row\'s menu button', async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: () => Promise.reject(new Error('not allowed')) },
    });
  });
  await page.goto('/');
  const summary = await choose(page, 'Share link');
  const dialog = page.getByRole('dialog', { name: 'Link to this call' });
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(summary).toBeFocused();
});
