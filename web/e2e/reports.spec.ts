import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Reports area. The seed sets no time zone, so the server refuses to
// make the report; the figures are answered by the test.

test.beforeEach(async ({ page }) => freshStorage(page));

const slots = (each: number, busy: number) => Array.from({ length: 288 }, (_, i) => (i >= 168 && i < 180 ? busy : each));
const TRAFFIC = {
  days: 120, from: '2031-03-03T06:00:00Z', until: '2031-07-01T05:00:00Z', time_zone: 'America/Chicago',
  weekdays: 86, weekend_days: 34,
  weekday: slots(0.5, 4), weekend: slots(0.25, 0.25),
  quietest_weekday: [{ start: '03:25', end: '04:25', calls_per_day: 1.2 }, { start: '05:00', end: '06:00', calls_per_day: 1.5 }],
  quietest_weekend: [{ start: '23:55', end: '00:55', calls_per_day: 0.8 }],
  calls: 12345, transcribed: 9876, no_speech: 543,
};

test("the report's refusal is shown in the server's words", async ({ page }) => {
  await signIn(page, '#reports');
  await expect(page.getByRole('alert')).toHaveText(/^The report could not be made: .+/);
});

test('the traffic report shows its totals, its charts, the quietest hours and every hour', async ({ page }) => {
  await page.route('**/api/admin/reports/traffic', (route) =>
    route.fulfill({ contentType: 'application/json', body: JSON.stringify(TRAFFIC) }));
  await signIn(page, '#reports');
  await expect(page.getByText('12,345 calls in the 120 days before today (86 weekdays, 34 weekend days); times in America/Chicago.')).toBeVisible();
  await expect(page.getByText('9,876 (80%)')).toBeVisible();
  await expect(page.getByText('543 (4%)')).toBeVisible();
  await expect(page.getByRole('figure', { name: 'Weekdays' })).toBeVisible();
  await expect(page.getByRole('figure', { name: 'Weekend days' })).toBeVisible();
  await expect(page.getByRole('list', { name: 'The quietest hours on weekdays' })).toContainText('03:25 to 04:25: 1.2 calls a day');
  await expect(page.getByRole('list', { name: 'The quietest hours on weekend days' })).toContainText('23:55 to 00:55: 0.8 calls a day');
  await page.getByText("Every hour's figures").click();
  const rows = page.getByRole('table', { name: 'Calls a day in each hour' }).locator('tbody tr');
  await expect(rows).toHaveCount(24);
  await expect(rows.nth(14)).toHaveText(/14:00\s*48\.0\s*3\.0/);
  await expect(rows.nth(0)).toHaveText(/00:00\s*6\.0\s*3\.0/);
});

for (const [theme, width] of [['light', 1280], ['dark', 360]] as const) {
  test(`the report has no accessibility fault and no sideways scroll in the ${theme} theme at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.addInitScript((t) => localStorage.setItem('scannerpulse.theme', t), theme);
    await page.route('**/api/admin/reports/traffic', (route) =>
      route.fulfill({ contentType: 'application/json', body: JSON.stringify(TRAFFIC) }));
    await signIn(page, '#reports');
    await page.getByText("Every hour's figures").click();
    await expect(page.getByRole('table', { name: 'Calls a day in each hour' })).toBeVisible();
    const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
    expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)).toBe(false);
  });
}
