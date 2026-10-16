import { expect, test } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Jobs area, against the three jobs e2e/seed.sql makes: a purge that
// failed, a correction that ended, and a queue job that waits on maintenance.
// Nothing runs jobs here, so a stop is answered by the test where it would
// change a job for good.

test.beforeEach(async ({ page }) => freshStorage(page));

test('the jobs are listed newest first, with their progress and why one waits', async ({ page }) => {
  await signIn(page, '#jobs');
  const jobs = page.getByRole('listitem');
  await expect(jobs).toHaveCount(3);
  await expect(jobs.nth(0)).toContainText('Queue past calls for transcription #3');
  await expect(jobs.nth(0)).toContainText('40 of 100');
  await expect(jobs.nth(0)).toContainText('Waiting maintenance is on');
  await expect(jobs.nth(0).getByRole('progressbar')).toHaveAttribute('value', '40');
  await expect(jobs.nth(1)).toContainText('done');
  await expect(jobs.nth(2)).toContainText('the holding folder is not there');
  // Only a job that has not ended can be stopped.
  await expect(page.getByRole('button', { name: /^Stop job/ })).toHaveCount(1);
});

test('the side menu counts the jobs under way', async ({ page }) => {
  await signIn(page);
  await expect(page.getByRole('navigation', { name: 'Admin areas' }).getByRole('link', { name: 'Jobs, 1 under way' })).toBeVisible();
});

test("a stop asks the server, says its answer and reads the list again", async ({ page }) => {
  let asked = '';
  await page.route('**/api/admin/jobs/3/stop', async (route) => {
    asked = route.request().headers()['x-csrf-token'] ?? '';
    await route.fulfill({ json: { id: 3, stopped: 'after its current step' } });
  });
  await signIn(page, '#jobs');
  const reads = page.waitForRequest((r) => r.url().endsWith('/api/admin/jobs') && r.method() === 'GET');
  await page.getByRole('button', { name: 'Stop job #3' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Stopped after its current step.' })).toBeVisible();
  await reads;
  expect(asked).not.toBe('');
});

test("a refused stop is shown beside its button, in the server's words", async ({ page }) => {
  await page.route('**/api/admin/jobs/3/stop', (route) =>
    route.fulfill({ status: 409, contentType: 'application/json', body: JSON.stringify({ error: 'conflict', detail: 'job 3 has ended' }) }),
  );
  await signIn(page, '#jobs');
  await page.getByRole('button', { name: 'Stop job #3' }).click();
  await expect(page.getByRole('alert')).toHaveText('The job was not stopped: job 3 has ended');
});
