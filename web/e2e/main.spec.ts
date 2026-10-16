import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';

// Against e2e/seed.sql: calls 1 to 30 in the last 90 minutes (call 1 the
// newest), 31 to 35 three days ago; call 7 has no audio file; call 5's text
// holds markup.

const rows = (page: Page) => page.locator('tbody tr');
const ids = (page: Page) => rows(page).evaluateAll((trs) => trs.map((tr) => Number((tr as HTMLElement).dataset.id)));
const playingRow = (page: Page) => page.locator('tr.playing');

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('e2e')) {
      localStorage.clear();
      sessionStorage.setItem('e2e', '1');
    }
  });
});

test('the first view is the last 24 hours, newest first', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await expect(page.getByText('30 calls')).toBeVisible();
  await page.getByLabel('All dates').check();
  await expect(rows(page)).toHaveCount(35);
  await expect(page).toHaveURL(/all=1/);
});

test('pages are read on the server, by size, by next and by number', async ({ page }) => {
  await page.goto('/?all=1');
  await page.getByLabel('Rows').selectOption('10');
  await expect(rows(page)).toHaveCount(10);
  await expect(page.getByText('of 4')).toBeVisible();
  const first = await ids(page);
  await page.getByRole('button', { name: 'Next' }).click();
  await expect.poll(() => ids(page)).not.toEqual(first);
  await page.getByLabel('Page number').fill('4');
  await page.getByLabel('Page number').press('Enter');
  await expect(rows(page)).toHaveCount(5);
  await expect(page.getByRole('button', { name: 'Next' })).toBeDisabled();
  await page.getByRole('button', { name: 'First' }).click();
  await expect.poll(() => ids(page)).toEqual(first);
  // The size is the listener's: it is still 10 after a reload.
  await page.reload();
  await expect(rows(page)).toHaveCount(10);
});

test('a talkgroup filter is in the URL, and the link reproduces it', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('Find a talkgroup').fill('ambul');
  await page.getByRole('checkbox', { name: 'Valley Ambulance' }).check();
  await expect(rows(page)).toHaveCount(10);
  await expect(page.locator('td.tg').filter({ hasNotText: 'Valley Ambulance' })).toHaveCount(0);
  const url = page.url();
  expect(url).toMatch(/talkgroups=\d+/);
  await page.goto(url);
  await expect(rows(page)).toHaveCount(10);
  await expect(page.getByRole('checkbox', { name: 'Valley Ambulance' })).toBeChecked();
});

test('a favorite asks for its talkgroups', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('Favorite').selectOption({ label: 'Fire and ambulance' });
  await expect(rows(page)).toHaveCount(20);
  await expect(page).toHaveURL(/favorite=\d+/);
});

test('text is shown as text, never as markup, and opens in full', async ({ page }) => {
  await page.goto('/');
  const row = page.locator('tr', { has: page.getByText('<b>not bold</b>') });
  await expect(row).toHaveCount(1);
  await expect(row.locator('b')).toHaveCount(0);
  const long = rows(page).filter({ hasText: 'Engine 9 ' });
  await long.getByRole('button', { name: 'More' }).click();
  await expect(long.getByRole('button', { name: 'Less' })).toHaveAttribute('aria-expanded', 'true');
});

test('the player steps with N and P, stops at the edges and passes over a call that fails', async ({ page }) => {
  await page.goto('/');
  // The calls are 2 s long: without continuous play none moves on by itself.
  await page.getByLabel('Continuous').uncheck();
  const all = await ids(page);
  await rows(page).nth(2).getByRole('button', { name: /^Play/ }).click();
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[2]));
  await page.keyboard.press('n');
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[1]));
  await page.keyboard.press('n');
  await page.keyboard.press('n');
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[0]));
  await expect(page.getByRole('button', { name: 'Newer call (N)' })).toBeDisabled();
  // Rows 6, 7, 8 are calls 6, 7 and 8 (newest first); call 7 has no file.
  await rows(page).nth(5).getByRole('button', { name: /^Play/ }).click();
  await page.keyboard.press('p');
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[7]));
  await expect(rows(page).nth(6)).toHaveClass(/played/);
  // Keys typed into a field are the field's.
  await page.getByLabel('Find a talkgroup').fill('np');
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[7]));
});

test('continuous play moves to the newer call when one ends', async ({ page }) => {
  await page.goto('/');
  const all = await ids(page);
  await rows(page).nth(4).getByRole('button', { name: /^Play/ }).click();
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[3]), { timeout: 10_000 });
});

test('the theme is chosen before the page draws, and kept', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: /^Theme/ }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.reload({ waitUntil: 'commit' });
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
});

for (const theme of ['light', 'dark']) {
  for (const width of [1280, 360]) {
    test(`no accessibility fault in the ${theme} theme at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 800 });
      await page.addInitScript((t) => localStorage.setItem('scannerpulse.theme', t), theme);
      await page.goto('/');
      await expect(rows(page)).toHaveCount(30);
      const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
      expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    });
  }
}

test('on a phone the filters open from the Filters button', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto('/');
  await expect(page.getByLabel('Find a talkgroup')).toBeHidden();
  await page.getByRole('button', { name: 'Filters' }).click();
  await expect(page.getByLabel('Find a talkgroup')).toBeVisible();
});
