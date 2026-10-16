import { expect, test, type Page } from '@playwright/test';

// Against e2e/seed.sql, as main.spec.ts: calls 1 to 30 in the last 90
// minutes. What the Export menu refuses with nothing chosen, and how a
// printout looks in the dark theme.

const rows = (page: Page) => page.locator('tbody tr');
const exportMenu = (page: Page) => page.locator('details.export');
const item = (page: Page, name: RegExp) => exportMenu(page).getByRole('button', { name });

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('e2e')) {
      localStorage.clear();
      sessionStorage.setItem('e2e', '1');
    }
  });
});

test('with selection on and nothing ticked, the page exports and call audio are off, and Excel still covers every match', async ({ page }) => {
  const requests: string[] = [];
  page.on('request', (request) => {
    if (request.url().includes('/api/export')) requests.push(request.url());
  });
  await page.addInitScript(() => {
    (window as unknown as { printed: number }).printed = 0;
    window.print = () => { (window as unknown as { printed: number }).printed += 1; };
  });
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await page.getByRole('button', { name: 'Select calls' }).click();
  await expect(page.locator('nav.pager .chosen')).toHaveText('0 selected');

  await exportMenu(page).locator('summary').click();
  const refused = [/^CSV/, /^PDF/, /^Print/, /^Call audio/];
  for (const name of refused) {
    await expect(item(page, name)).toBeDisabled();
    // Whatever is tried on a disabled item, nothing is asked of the server.
    await item(page, name).click({ force: true });
  }
  await expect(item(page, /^Excel/)).toBeEnabled();
  expect(requests).toEqual([]);
  expect(await page.evaluate(() => (window as unknown as { printed: number }).printed)).toBe(0);

  // One call ticked turns them on again.
  await page.keyboard.press('Escape');
  await page.locator('tbody td.c-select input').nth(0).check();
  await exportMenu(page).locator('summary').click();
  for (const name of refused) await expect(item(page, name)).toBeEnabled();
});

for (const how of ['chosen', 'the device\'s']) {
  test(`a printout in the dark theme (${how}) is dark text on white`, async ({ page }) => {
    if (how === 'chosen') {
      await page.addInitScript(() => localStorage.setItem('scannerpulse.theme', 'dark'));
    } else {
      await page.emulateMedia({ colorScheme: 'dark' });
    }
    await page.goto('/');
    if (how === 'chosen') await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
    else await expect(page.locator('html')).not.toHaveAttribute('data-theme', /./);
    await expect(rows(page)).toHaveCount(30);
    expect(await page.evaluate(() => getComputedStyle(document.body).color)).not.toBe('rgb(0, 0, 0)');
    await page.emulateMedia({ media: 'print', colorScheme: 'dark' });
    const found = await page.evaluate(() => {
      const channels = (css: string) => (css.match(/[\d.]+/g) ?? []).map(Number);
      const luminance = (css: string) => {
        const [r, g, b] = channels(css).slice(0, 3).map((v) => {
          const s = v / 255;
          return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
        });
        return 0.2126 * r + 0.7152 * g + 0.0722 * b;
      };
      // The first opaque background behind an element, white if there is none.
      const behind = (el: Element) => {
        for (let at: Element | null = el; at; at = at.parentElement) {
          const css = getComputedStyle(at).backgroundColor;
          const c = channels(css);
          if (c.length === 3 || (c.length === 4 && c[3] > 0)) return css;
        }
        return 'rgb(255, 255, 255)';
      };
      const ratio = (el: Element) => {
        // Dark text on a light ground: the ground is the lighter of the two.
        return (luminance(behind(el)) + 0.05) / (luminance(getComputedStyle(el).color) + 0.05);
      };
      const header = [...document.querySelectorAll('thead th')].find((th) => th.textContent?.trim());
      const parts: Record<string, Element | null | undefined> = {
        talkgroup: document.querySelector('tbody tr td.tg'),
        text: document.querySelector('tbody tr .snippet'),
        length: document.querySelector('tbody tr .secs'),
        header,
      };
      return {
        ratios: Object.fromEntries(Object.entries(parts).map(([k, el]) => [k, el ? ratio(el) : 0])),
        page: luminance(behind(document.body)),
      };
    });
    // The page itself is white, and every kind of text keeps 4.5:1 or more on its background.
    expect(found.page).toBeCloseTo(1, 2);
    for (const [part, ratio] of Object.entries(found.ratios)) expect(ratio, part).toBeGreaterThanOrEqual(4.5);
  });
}
