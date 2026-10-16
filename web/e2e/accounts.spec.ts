import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Accounts area. Each run adds an account of its own name, which stays
// (enabled) in the test database.

test.beforeEach(async ({ page }) => freshStorage(page));

const card = (page: Page, login: string) => page.getByRole('listitem').filter({ has: page.getByRole('heading', { name: new RegExp(`^${login}( \\(you\\))?$`) }) });

test('the admins are listed with how they sign in', async ({ page }) => {
  await signIn(page, '#accounts');
  const me = card(page, 'e2e-admin');
  await expect(me.getByRole('heading')).toHaveText('e2e-admin (you)');
  await expect(me).toContainText('Signs in with a password · enabled');
  await expect(me.getByRole('button', { name: 'Disable e2e-admin (signs you out)' })).toBeVisible();
});

test('an admin is added, given a new password, disabled and enabled again', async ({ page }) => {
  const login = `e2e-${Date.now()}`;
  await signIn(page, '#accounts');
  const add = page.locator('form.password').last();
  await add.getByLabel('Name').fill(login);
  await add.getByLabel('Password', { exact: true }).fill('another e2e password');
  await add.getByLabel('Password again').fill('another e2e password');
  await add.getByRole('button', { name: 'Add the admin' }).click();
  const row = card(page, login);
  await expect(row).toContainText('enabled');
  await row.getByRole('button', { name: `New password for ${login}` }).click();
  await row.getByLabel('Password', { exact: true }).fill('a third e2e password');
  await row.getByLabel('Password again').fill('a third e2e password');
  await row.getByRole('button', { name: 'Save the password' }).click();
  await expect(page.getByRole('status').filter({ hasText: `${login} has a new password.` })).toBeVisible();
  await row.getByRole('button', { name: `Disable ${login}` }).click();
  await expect(row).toContainText('disabled');
  await row.getByRole('button', { name: `Enable ${login}` }).click();
  await expect(row).toContainText('enabled');
});

test("a refused account is shown beside its form, in the server's words", async ({ page }) => {
  await signIn(page, '#accounts');
  const add = page.locator('form.password').last();
  await add.getByLabel('Name').fill('someone');
  await add.getByLabel('Password', { exact: true }).fill('short');
  await add.getByLabel('Password again').fill('shorts');
  await add.getByRole('button', { name: 'Add the admin' }).click();
  await expect(add.getByRole('alert')).toHaveText('Not saved: The two passwords differ.');
  await add.getByLabel('Password again').fill('short');
  await add.getByRole('button', { name: 'Add the admin' }).click();
  await expect(add.getByRole('alert')).toHaveText('Not saved: a password has at least 12 characters');
});

test("the last enabled admin's refusal shows beside its account", async ({ page }) => {
  await page.route('**/api/admin/accounts/e2e-admin/disable', (route) =>
    route.fulfill({ status: 409, contentType: 'application/json', body: JSON.stringify({ error: 'conflict', detail: 'this is the last enabled admin: enable or add another first' }) }),
  );
  await signIn(page, '#accounts');
  const me = card(page, 'e2e-admin');
  await me.getByRole('button', { name: /^Disable e2e-admin/ }).click();
  await expect(me.getByRole('alert')).toHaveText('Not changed: this is the last enabled admin: enable or add another first');
});

test('the sign-in settings are beside the accounts', async ({ page }) => {
  await signIn(page, '#accounts');
  for (const key of ['auth.password_signin', 'oidc.issuer', 'oidc.client_id', 'oidc.admin_group', 'oidc.redirect_url']) {
    await expect(page.locator('form.setting').filter({ has: page.getByText(key, { exact: true }) })).toBeVisible();
  }
});
