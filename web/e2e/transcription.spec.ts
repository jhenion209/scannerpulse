import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Transcription area, against e2e/seed.sql, where transcription is off
// (`transcription.enabled` is not set). A switch changed here is put back;
// a queue job is answered by the test, since nothing runs jobs here and the
// Jobs tests count the seeded ones.

test.beforeEach(async ({ page }) => freshStorage(page));

const json = (body: unknown, status = 200) => ({ status, contentType: 'application/json', body: JSON.stringify(body) });
const switchOf = (page: Page, name: RegExp) => page.getByRole('checkbox', { name });

test("each talkgroup's switch turns its transcription on and off, and is kept", async ({ page }) => {
  await signIn(page, '#transcription');
  const fire = switchOf(page, /^Transcribe Metro Fire Dispatch/);
  await expect(fire).not.toBeChecked();
  const saved = () => page.waitForResponse((r) => r.url().endsWith('/transcribe') && r.request().method() === 'PUT');
  await Promise.all([saved(), fire.check()]);
  await page.reload();
  await expect(switchOf(page, /^Transcribe Metro Fire Dispatch/)).toBeChecked();
  await Promise.all([saved(), switchOf(page, /^Transcribe Metro Fire Dispatch/).uncheck()]);
  await page.reload();
  await expect(switchOf(page, /^Transcribe Metro Fire Dispatch/)).not.toBeChecked();
});

test('the list finds a talkgroup by name or number', async ({ page }) => {
  await signIn(page, '#transcription');
  const rows = page.getByRole('table', { name: "Each talkgroup's transcription switch" }).locator('tbody tr');
  await expect(rows).toHaveCount(3);
  await page.getByLabel('Find a talkgroup').fill('201');
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText('Valley Ambulance');
});

test("a queue job is counted first, refused in the server's words, and asked for only after", async ({ page }) => {
  await signIn(page, '#transcription');
  // The seed sets neither a time zone nor transcription: the server refuses to count.
  await page.getByRole('button', { name: 'Count the calls' }).click();
  await expect(page.getByRole('alert')).toHaveText(/^Not queued: .+/);
  await expect(page.getByRole('button', { name: /^Queue / })).toHaveCount(0);

  let asked: unknown = null;
  await page.route('**/api/admin/jobs/queue*', async (route) => {
    if (route.request().method() === 'GET') return route.fulfill(json({ count: 42, min_ms: 1000 }));
    asked = route.request().postDataJSON();
    return route.fulfill(json({ id: 7, count: 42 }, 201));
  });
  await page.getByLabel('Talkgroup', { exact: true }).selectOption({ label: 'Valley Ambulance · Valley 201' });
  await page.getByLabel('From').fill('2031-07-01');
  await page.getByRole('button', { name: 'Count the calls' }).click();
  await expect(page.getByText('42 calls would be queued (calls shorter than 1 s are left out).')).toBeVisible();
  // A changed field makes the count stale.
  await page.getByLabel('To').fill('2031-07-02');
  await expect(page.getByRole('button', { name: /^Queue / })).toHaveCount(0);
  await page.getByRole('button', { name: 'Count the calls' }).click();
  await page.getByRole('button', { name: 'Queue 42 calls' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Job #7 queues 42 calls' })).toBeVisible();
  expect(asked).toMatchObject({ from: '2031-07-01', to: '2031-07-02' });
  expect((asked as { talkgroup_id: number }).talkgroup_id).toBeGreaterThan(0);
});

test('the failed calls are put back in the queue', async ({ page }) => {
  await signIn(page, '#transcription');
  await page.getByRole('button', { name: 'Try the failed calls again' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'No failed call to try again.' })).toBeVisible();
});
