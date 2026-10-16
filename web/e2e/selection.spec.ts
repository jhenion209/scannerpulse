import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Locator, type Page } from '@playwright/test';

// Against e2e/seed.sql: calls 1 to 30 in the last 90 minutes (call 1 the
// newest). Selection mode on a phone, and a chosen call that was played.

const rows = (page: Page) => page.locator('tbody tr');

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('e2e')) {
      localStorage.clear();
      sessionStorage.setItem('e2e', '1');
    }
  });
});

// A box inside a clipped 1 px header is "visible" to Playwright (it has a
// size), so ask what the screen shows at its middle.
const shownOnScreen = (box: Locator) =>
  box.evaluate((el) => {
    const r = el.getBoundingClientRect();
    return r.width >= 12 && r.height >= 12 && document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2) === el;
  });

test('on a phone the box for the whole page is on screen, the first Tab stop, and ticks the page', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await page.getByRole('button', { name: 'Select calls' }).click();
  const all = page.getByLabel('Select every call on this page');
  // The last control before the list is the page size; the next Tab enters the list.
  await page.getByLabel('Rows').focus();
  await page.keyboard.press('Tab');
  await expect(all).toBeFocused();
  expect(await shownOnScreen(all)).toBe(true);
  await page.keyboard.press('Space');
  await expect(page.getByText('30 selected', { exact: true })).toBeVisible();
  await expect(all).toBeChecked();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(360);
});

test('a played call that is chosen keeps readable text in the light theme', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.addInitScript(() => localStorage.setItem('scannerpulse.theme', 'light'));
  await page.goto('/');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await expect(rows(page)).toHaveCount(30);
  // Play one call, then another: the first stays marked played.
  await rows(page).nth(2).getByRole('button', { name: /^Play/ }).click();
  await rows(page).nth(3).getByRole('button', { name: /^Play/ }).click();
  await expect(rows(page).nth(2)).toHaveClass(/played/);
  await page.getByRole('button', { name: 'Select calls' }).click();
  await rows(page).nth(2).locator('td.c-select input').check();
  await expect(rows(page).nth(2)).toHaveClass(/selected/);
  const result = await new AxeBuilder({ page }).include('table.calls').withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
  expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
  // The ratio itself (axe may leave a mixed colour for review): text over tint.
  const ratio = await rows(page).nth(2).locator('td.c-time').evaluate((td) => {
    const px = (css: string) => {
      const c = document.createElement('canvas').getContext('2d', { willReadFrequently: true })!;
      c.fillStyle = css;
      c.fillRect(0, 0, 1, 1);
      return Array.from(c.getImageData(0, 0, 1, 1).data.slice(0, 3));
    };
    const lum = ([r, g, b]: number[]) =>
      [r, g, b].map((v) => (v / 255 <= 0.03928 ? v / 255 / 12.92 : ((v / 255 + 0.055) / 1.055) ** 2.4)).reduce((s, v, i) => s + v * [0.2126, 0.7152, 0.0722][i], 0);
    const style = getComputedStyle(td);
    const [a, b] = [lum(px(style.color)), lum(px(style.backgroundColor))].sort((x, y) => y - x);
    return (a + 0.05) / (b + 0.05);
  });
  expect(ratio).toBeGreaterThanOrEqual(4.5);
});
