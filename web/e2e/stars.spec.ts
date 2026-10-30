import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';

// Against e2e/seed.sql: calls 1 to 30 in the last 90 minutes (call 1 the
// newest, so row n is call n); the odd calls have text, the even ones none.

const rows = (page: Page) => page.locator('tbody tr');
const playingRow = (page: Page) => page.locator('tr.playing');
const star = (page: Page, row: number, n: number) => rows(page).nth(row).getByRole('button', { name: `Rate ${n} of 5` });

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('e2e')) {
      localStorage.clear();
      sessionStorage.setItem('e2e', '1');
    }
  });
});

test('one tap rates a call, a tap on the same star clears it, and the row does not play', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);

  const saved = page.waitForResponse((r) => r.url().includes('/rating') && r.request().method() === 'PUT');
  await star(page, 0, 4).click();
  expect((await saved).status()).toBe(200);
  await expect(star(page, 0, 4)).toHaveAttribute('aria-pressed', 'true');
  await expect(star(page, 0, 3)).toHaveClass(/on/);
  await expect(star(page, 0, 5)).not.toHaveClass(/on/);
  await expect(playingRow(page)).toHaveCount(0);

  // The stars given are still shown after a reload.
  await page.reload();
  await expect(rows(page)).toHaveCount(30);
  await expect(star(page, 0, 4)).toHaveAttribute('aria-pressed', 'true');

  // Another star changes it; the same star again clears it.
  await star(page, 0, 2).click();
  await expect(star(page, 0, 2)).toHaveAttribute('aria-pressed', 'true');
  await expect(star(page, 0, 4)).toHaveAttribute('aria-pressed', 'false');
  const cleared = page.waitForResponse((r) => r.url().includes('/rating') && r.request().method() === 'DELETE');
  await star(page, 0, 2).click();
  expect((await cleared).status()).toBe(200);
  await expect(star(page, 0, 2)).toHaveAttribute('aria-pressed', 'false');
  await expect(rows(page).nth(0).locator('button.on')).toHaveCount(0);
});

test('a call with no text has no stars', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const stars = (row: number) => rows(page).nth(row).getByRole('group', { name: /Rate the text of/ });
  await expect(stars(1)).toHaveCount(0);
  await expect(stars(0)).toHaveCount(1);
});

test('a refused rating puts the old stars back and says so', async ({ page }) => {
  await page.route('**/api/calls/*/rating', (route) =>
    route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ error: 'unavailable', detail: 'the site is under maintenance: rating is paused' }) }),
  );
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await star(page, 0, 5).click();
  await expect(star(page, 0, 5)).toHaveAttribute('aria-pressed', 'false');
  await expect(page.getByText(/rating was not saved/i)).toBeVisible();
});

test('the player bar rates the playing call', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await rows(page).nth(0).locator('td.c-talkgroup').click();
  await expect(playingRow(page)).toHaveCount(1);
  const bar = page.locator('.player').getByRole('group', { name: /Rate the text of/ });
  await bar.getByRole('button', { name: 'Rate 5 of 5' }).click();
  await expect(bar.getByRole('button', { name: 'Rate 5 of 5' })).toHaveAttribute('aria-pressed', 'true');
  // The same call's row shows it too.
  await expect(star(page, 0, 5)).toHaveAttribute('aria-pressed', 'true');
});

for (const theme of ['light', 'dark'] as const) {
  for (const width of [1280, 360]) {
    test(`the stars pass the accessibility check at ${width} px, ${theme}`, async ({ page }) => {
      await page.addInitScript((t) => localStorage.setItem('scannerpulse.theme', t), theme);
      await page.setViewportSize({ width, height: 800 });
      await page.goto('/');
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      await expect(rows(page)).toHaveCount(30);
      await star(page, 0, 3).click();
      const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
      expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    });
  }
}
