import { expect, test, type Page } from '@playwright/test';

// Every time on the page is the site's. The seeded site has no time zone
// setting, so it is UTC; this browser is in Tokyo, nine hours ahead.

const rows = (page: Page) => page.locator('tbody tr');

test.use({ timezoneId: 'Asia/Tokyo' });

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('e2e')) {
      localStorage.clear();
      sessionStorage.setItem('e2e', '1');
    }
  });
});

test("a browser in another zone shows the site's times", async ({ page }) => {
  const newest = (await (await page.request.get('/api/calls?size=10')).json()).calls[0];
  const utc = new Date(newest.started_at).toISOString().slice(11, 19);
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await expect(rows(page).first().locator('.c-time')).toContainText(utc);
});

test("a date typed in the filters is the site's", async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const asked = page.waitForRequest((r) => r.url().includes('/api/calls?') && new URL(r.url()).searchParams.has('until'));
  await page.getByRole('complementary', { name: 'Filters' }).getByLabel('To', { exact: true }).fill('2031-01-01T08:00');
  expect(new URL((await asked).url()).searchParams.get('until')).toBe('2031-01-01T08:00:00.000Z');
});
