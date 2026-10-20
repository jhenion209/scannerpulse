import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Recorders area. The seed's recorder `live` is watched. A test adds a
// recorder in /tmp, which the server's machine has and its account may write
// in, wherever the browser runs; no ingest runs beside these tests, so
// nothing is taken from it. Each recorder added has a name of its own, since
// a removed recorder keeps its name.

test.beforeEach(async ({ page }) => freshStorage(page));

const card = (page: Page, name: string) =>
  page.getByRole('listitem').filter({ has: page.getByRole('heading', { name: new RegExp(`^${name} `) }) });

const unique = (stem: string) => `${stem}-${Date.now().toString(36)}`;

test('a recorder is added taken in when asked, brought in, and removed', async ({ page }) => {
  const name = unique('north');
  await signIn(page, '#recorders');
  await expect(page.getByRole('heading', { name: 'Recorders', level: 1 })).toBeVisible();
  await expect(card(page, 'live').getByText('Watched')).toBeVisible();
  await page.getByLabel('Name').fill(name);
  await page.getByLabel('Folder').fill('/tmp');
  await page.getByRole('radio', { name: 'only when asked' }).check();
  await page.getByRole('button', { name: 'Add the recorder' }).click();
  await expect(card(page, name).getByText('When asked')).toBeVisible();
  await card(page, name).getByRole('button', { name: `Bring in now ${name}` }).click();
  await expect(page.getByRole('status').filter({ hasText: `${name}'s files are brought in` })).toBeVisible();
  // No ingest runs beside these tests: the request waits to be listed.
  await expect(card(page, name).getByText('Bringing in')).toBeVisible();
  await card(page, name).getByRole('button', { name: `Remove… ${name}` }).click();
  await card(page, name).getByRole('button', { name: `Remove ${name}`, exact: true }).click();
  await expect(card(page, name)).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: `${name} is removed` })).toBeVisible();
});

test("a folder that cannot be used is refused in the server's words, and nothing is added", async ({ page }) => {
  await signIn(page, '#recorders');
  await page.getByLabel('Name').fill(unique('west'));
  await page.getByLabel('Folder').fill('/no/such/folder');
  await page.getByRole('button', { name: 'Add the recorder' }).click();
  await expect(page.getByRole('alert')).toHaveText(
    "Not added: /no/such/folder does not exist on this machine: mount the recorder's share there first",
  );
  await expect(page.getByRole('listitem')).toHaveCount(1);
});

test('a watched recorder is paused and watched again', async ({ page }) => {
  await signIn(page, '#recorders');
  await card(page, 'live').getByRole('button', { name: 'Pause live' }).click();
  await expect(card(page, 'live').getByText('Paused')).toBeVisible();
  await card(page, 'live').getByRole('button', { name: 'Watch live' }).click();
  await expect(card(page, 'live').getByText('Watched')).toBeVisible();
});
