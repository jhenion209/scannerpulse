import { expect, test } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Status area's table of what runs now: the jobs that run or wait, and
// the root helper's requests that have not ended, each with who started it.

test.beforeEach(async ({ page }) => freshStorage(page));

const job = (over: Record<string, unknown>) => ({
  id: 1, kind: 'queue', params: {}, state: 'queued', done: 0, total: null, message: '', started_by: 'ann',
  created_at: '2031-07-01T10:00:00Z', started_at: null, finished_at: null, updated_at: null, host: null,
  stop_requested: false, ...over,
});

for (const width of [1280, 360]) {
  test(`Status lists what runs now and who started it, at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.route('**/api/admin/jobs', (route) =>
      route.fulfill({
        json: [
          job({ id: 9, kind: 'purge_before', started_by: 'root on sp-a' }),
          job({ id: 8, kind: 'move', state: 'running', done: 2, total: 5, started_by: 'cy', started_at: '2031-07-01T10:01:00Z' }),
          job({ id: 7, state: 'done', finished_at: '2031-07-01T09:00:00Z' }),
        ],
      }),
    );
    await page.route('**/api/admin/updates', (route) =>
      route.fulfill({
        json: {
          running: '0.0.0', machines: [], newest: null, check: false, helper: null, open: true,
          requests: {
            archive_switch: { id: 5, kind: 'archive_switch', requested_by: 'ann', requested_at: '2031-07-01T10:02:00Z', started_at: '2031-07-01T10:02:05Z', finished_at: null, host: 'sp-a', ok: null, said: '' },
          },
        },
      }),
    );
    await signIn(page, '#status');
    const table = page.getByRole('table', { name: "The jobs and the root helper's requests that run or wait" });
    const rows = table.locator('tbody tr');
    await expect(rows).toHaveCount(3);
    await expect(rows.nth(0)).toContainText('Job 8');
    await expect(rows.nth(0)).toContainText('Move a date range');
    await expect(rows.nth(0)).toContainText('2 of 5');
    await expect(rows.nth(0)).toContainText('cy');
    await expect(rows.nth(1)).toContainText('root on sp-a');
    await expect(rows.nth(1)).toContainText('waiting');
    await expect(rows.nth(2)).toContainText("The archive's switch");
    await expect(rows.nth(2)).toContainText('ann');
    await expect(rows.nth(0).getByRole('link', { name: 'Job 8' })).toHaveAttribute('href', '#jobs');
    await expect(rows.nth(2).getByRole('link', { name: 'Request 5' })).toHaveAttribute('href', '#layout');
    const wide = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
    expect(wide).toBe(false);
  });
}

test('Status says when nothing runs or waits', async ({ page }) => {
  await page.route('**/api/admin/jobs', (route) => route.fulfill({ json: [job({ state: 'done' })] }));
  await page.route('**/api/admin/updates', (route) =>
    route.fulfill({ json: { running: '0.0.0', machines: [], newest: null, check: false, helper: null, open: false, requests: {} } }),
  );
  await signIn(page, '#status');
  await expect(page.getByText('Nothing runs or waits: no job, and no request of the root helper.')).toBeVisible();
});
