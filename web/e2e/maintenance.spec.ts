import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Maintenance area. Maintenance is put back off after each test, so
// that the other tests' saves are not refused.

test.beforeEach(async ({ page }) => freshStorage(page));
test.afterEach(async ({ page }) => {
  await page.evaluate(async () => {
    const me = await fetch('/api/auth/me');
    if (!me.ok) return;
    const { csrf } = await me.json();
    await fetch('/api/admin/settings/maintenance.enabled', {
      method: 'PUT',
      headers: { 'content-type': 'application/json', 'x-csrf-token': csrf },
      body: JSON.stringify({ value: false }),
    });
  });
});

const publicStatus = (page: Page) => page.evaluate(() => fetch('/api/status').then((r) => r.json()));

test('maintenance is turned on and off from its area', async ({ page }) => {
  await signIn(page, '#maintenance');
  await expect(page.getByRole('heading', { name: 'Maintenance', level: 1 })).toBeVisible();
  await expect(page.getByText('Maintenance is off.')).toBeVisible();
  await page.getByRole('button', { name: 'Turn maintenance on' }).click();
  await expect(page.getByText('Maintenance is on.')).toBeVisible();
  expect((await publicStatus(page)).maintenance).toBe(true);
  await page.getByRole('button', { name: 'Turn maintenance off' }).click();
  await expect(page.getByText('Maintenance is off.')).toBeVisible();
  expect((await publicStatus(page)).maintenance).toBe(false);
});

test("a refused change is shown beside its button, in the server's words", async ({ page }) => {
  await page.route('**/api/admin/settings/maintenance.enabled', (route) =>
    route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ error: 'unavailable', detail: 'the database is a standby' }) }),
  );
  await signIn(page, '#maintenance');
  await page.getByRole('button', { name: 'Turn maintenance on' }).click();
  await expect(page.getByRole('alert')).toHaveText('Maintenance was not changed: the database is a standby');
  await expect(page.getByText('Maintenance is off.')).toBeVisible();
});

test('rescan asks every ingest to list its sources', async ({ page }) => {
  await signIn(page, '#maintenance');
  await page.getByRole('button', { name: 'Rescan now' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Every running ingest lists its sources now.' })).toBeVisible();
});

test('the queue shows how far it has drained, read every 15 seconds', async ({ page }) => {
  await page.clock.install();
  let queue = 200;
  await page.route('**/api/status', async (route) => {
    const response = await route.fetch();
    await route.fulfill({ response, json: { ...(await response.json()), queue, transcription: true, eta_seconds: queue * 3 } });
  });
  await signIn(page, '#maintenance');
  await expect(page.getByText('200 calls waiting, done in about 10 min.')).toBeVisible();
  queue = 50;
  await page.clock.fastForward('00:15');
  await expect(page.getByText('50 calls waiting, done in about 3 min.')).toBeVisible();
  await expect(page.getByRole('progressbar', { name: 'Drained since 200 were waiting' })).toHaveAttribute('value', '75');
  // Rounded down: 100 % only once no call waits.
  queue = 1;
  await page.clock.fastForward('00:15');
  await expect(page.getByRole('progressbar', { name: 'Drained since 200 were waiting' })).toHaveAttribute('value', '99');
});

test('while maintenance is on, the queue says it is paused instead of when it ends', async ({ page }) => {
  await page.route('**/api/status', async (route) => {
    const response = await route.fetch();
    await route.fulfill({ response, json: { ...(await response.json()), queue: 40, transcription: true, eta_seconds: 120, maintenance: true } });
  });
  await signIn(page, '#maintenance');
  await expect(page.getByText('40 calls waiting; paused by maintenance.')).toBeVisible();
});

test('the side menu opens the area, and the address names it', async ({ page }) => {
  await signIn(page);
  await page.getByRole('navigation', { name: 'Admin areas' }).getByRole('link', { name: 'Maintenance' }).click();
  await expect(page).toHaveURL(/#maintenance$/);
  await expect(page.getByRole('heading', { name: 'Maintenance', level: 1 })).toBeVisible();
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Status', level: 1 })).toBeVisible();
});
