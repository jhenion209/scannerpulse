import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Favorites and Viewers areas. A list made here is deleted again by the
// test that makes it.

test.beforeEach(async ({ page }) => freshStorage(page));

const list = (page: Page, name: string) => page.getByRole('listitem').filter({ has: page.getByRole('heading', { name, exact: true }) });

test("the seeded list shows its talkgroups by name", async ({ page }) => {
  await signIn(page, '#favorites');
  await expect(list(page, 'Fire and ambulance')).toContainText('Metro Fire Dispatch · Metro, Valley Ambulance · Valley');
});

test('a list is made, changed, chosen on the main page, and deleted after a second click', async ({ page }) => {
  const name = `E2E list ${Date.now()}`;
  await signIn(page, '#favorites');
  const editor = page.locator('form.editor').last();
  await editor.getByLabel('Name').fill(name);
  await editor.getByRole('checkbox', { name: /Metro Fire Dispatch/ }).check();
  await editor.getByRole('button', { name: 'Add the list' }).click();
  await expect(list(page, name)).toContainText('Metro Fire Dispatch · Metro');
  await list(page, name).getByRole('button', { name: `Edit ${name}` }).click();
  await list(page, name).getByRole('checkbox', { name: /Valley Ambulance/ }).check();
  await list(page, name).getByRole('button', { name: `Save ${name}` }).click();
  await expect(list(page, name)).toContainText('Metro Fire Dispatch · Metro, Valley Ambulance · Valley');

  const main = await page.context().newPage();
  await main.goto('/');
  await expect(main.getByLabel('Favorite').locator('option', { hasText: name })).toHaveCount(1);
  await main.close();

  await list(page, name).getByRole('button', { name: `Delete ${name}` }).click();
  await list(page, name).getByRole('button', { name: `Delete ${name} for good` }).click();
  await expect(list(page, name)).toHaveCount(0);
});

test("a list without a name is not sent, and a refusal is in the server's words", async ({ page }) => {
  await signIn(page, '#favorites');
  const editor = page.locator('form.editor').last();
  await editor.getByRole('button', { name: 'Add the list' }).click();
  await expect(editor.getByRole('alert')).toHaveText('Not saved: Name the list first.');
  await page.route('**/api/admin/favorites/*', (route) =>
    route.fulfill({ status: 400, contentType: 'application/json', body: JSON.stringify({ error: 'bad request', detail: 'a list holds at most 500 talkgroups' }) }),
  );
  await editor.getByLabel('Name').fill('Too long a list');
  await editor.getByRole('button', { name: 'Add the list' }).click();
  await expect(editor.getByRole('alert')).toHaveText('Not saved: a list holds at most 500 talkgroups');
});

// A save is answered here, so a test that fails never changes the database.
async function recordSaves(page: Page) {
  const saves: string[] = [];
  await page.route('**/api/admin/favorites/*', (route) => {
    if (route.request().method() !== 'PUT') return route.continue();
    saves.push(route.request().url());
    return route.fulfill({ status: 200, contentType: 'application/json', body: '{}' });
  });
  return saves;
}

test('Enter in the talkgroup search does not save the list', async ({ page }) => {
  const saves = await recordSaves(page);
  await signIn(page, '#favorites');
  const editor = page.locator('form.editor').last();
  await editor.getByLabel('Name').fill('Enter must not save');
  await editor.getByLabel('Find a talkgroup').fill('Metro');
  await editor.getByLabel('Find a talkgroup').press('Enter');
  await page.waitForTimeout(500);
  expect(saves).toEqual([]);
  await expect(editor.getByLabel('Name')).toHaveValue('Enter must not save');
});

test('a new list is not saved over a list of the same name', async ({ page }) => {
  const saves = await recordSaves(page);
  await signIn(page, '#favorites');
  const editor = page.locator('form.editor').last();
  await editor.getByLabel('Name').fill('Fire and ambulance');
  await editor.getByRole('checkbox', { name: /Metro Fire Dispatch/ }).check();
  await editor.getByRole('button', { name: 'Add the list' }).click();
  await expect(editor.getByRole('alert')).toHaveText('Not saved: A list named Fire and ambulance exists: use its Edit button.');
  expect(saves).toEqual([]);
});

test('a new list is not saved while the lists are not read', async ({ page }) => {
  const saves = await recordSaves(page);
  await page.route('**/api/favorites', (route) =>
    route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ error: 'unavailable', detail: 'the database does not answer' }) }),
  );
  await signIn(page, '#favorites');
  // The talkgroups are read together with the lists, so none can be ticked here.
  const editor = page.locator('form.editor').last();
  await editor.getByLabel('Name').fill('Fire and ambulance');
  await editor.getByRole('button', { name: 'Add the list' }).click();
  await expect(editor.getByRole('alert')).toHaveText('Not saved: The lists could not be read yet: reload the page before adding one.');
  expect(saves).toEqual([]);
});

test('the viewers are the pages open on the calls, with the admin who opened one', async ({ page }) => {
  await signIn(page);
  const main = await page.context().newPage();
  const live = main.waitForEvent('websocket');
  await main.goto('/');
  await live;
  await page.evaluate(() => (location.hash = '#viewers'));
  // The page's row is written just after its feed opens: read until it shows.
  const mine = page.getByRole('table', { name: 'The open pages' }).locator('tbody tr').filter({ hasText: 'e2e-admin' });
  await expect(async () => {
    await page.getByRole('button', { name: 'Read now' }).click();
    await expect(mine.first()).toBeVisible({ timeout: 1000 });
  }).toPass();
  await main.close();
});
