import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The admin page: signing in and out, the side menu and the Status area.
// Against e2e/seed.sql, where ingest has never run.

test.beforeEach(async ({ page }) => freshStorage(page));

const refuse = (status: number, detail: string) => ({
  status,
  contentType: 'application/json',
  body: JSON.stringify({ error: 'refused', detail }),
});
const statusHeading = (page: Page) => page.getByRole('heading', { name: 'Status', level: 1 });

test("the admin page asks to sign in, and shows a refusal in the server's words", async ({ page }) => {
  await page.route('**/api/auth/login', (route) => route.fulfill(refuse(401, 'the name or the password is wrong')));
  await page.goto('/admin');
  await page.getByLabel('Name').fill('someone');
  await page.getByLabel('Password').fill('not the password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('alert')).toHaveText('the name or the password is wrong');
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
});

test('an admin signs in, sees the status, and signs out', async ({ page }) => {
  await signIn(page);
  await expect(statusHeading(page)).toBeVisible();
  await expect(page.getByText('Ingest has never run against this database.')).toBeVisible();
  await expect(page.getByRole('table', { name: 'The newest calls' }).locator('tbody tr')).toHaveCount(20);
  await expect(page.getByRole('banner')).toContainText('e2e-admin');
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
  await page.reload();
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
});

test('single sign-on shows when it is set up, and no way in says how to turn one on', async ({ page }) => {
  let methods = { password: false, oidc: true };
  await page.route('**/api/auth/methods', (route) => route.fulfill({ json: methods }));
  await page.goto('/admin');
  await expect(page.getByRole('link', { name: 'Sign in with single sign-on' })).toHaveAttribute('href', '/api/auth/oidc');
  await expect(page.getByLabel('Password')).toHaveCount(0);
  methods = { password: false, oidc: false };
  await page.reload();
  await expect(page.getByRole('alert')).toContainText('scannerpulse admin password-signin on');
});

test('the status is read again every 15 seconds, and a failed read keeps the last one', async ({ page }) => {
  await page.clock.install();
  let reads = 0;
  let failing = false;
  await page.route('**/api/admin/status', async (route) => {
    reads += 1;
    if (failing) return route.fulfill(refuse(503, 'the database does not answer'));
    return route.continue();
  });
  await signIn(page);
  await expect(page.getByRole('heading', { name: 'Newest calls' })).toBeVisible();
  const before = reads;
  await page.clock.fastForward('00:15');
  await expect.poll(() => reads).toBe(before + 1);
  failing = true;
  await page.getByRole('button', { name: 'Read now' }).click();
  await expect(page.getByRole('alert')).toHaveText('The status could not be read: the database does not answer');
  await expect(page.getByRole('heading', { name: 'Newest calls' })).toBeVisible();
});

test('a session that ends brings the sign-in form back', async ({ page }) => {
  await signIn(page);
  await expect(statusHeading(page)).toBeVisible();
  await page.route('**/api/admin/status', (route) => route.fulfill(refuse(401, 'sign in as an admin first')));
  await page.getByRole('button', { name: 'Read now' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'The session has ended' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
});

test('on a phone the areas open from the Menu button', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 760 });
  await signIn(page);
  const menu = page.getByRole('navigation', { name: 'Admin areas' });
  await expect(menu).toBeHidden();
  await page.getByRole('button', { name: 'Menu' }).click();
  await expect(menu).toBeVisible();
  await menu.getByRole('link', { name: 'Status' }).click();
  await expect(menu).toBeHidden();
  const wide = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
  expect(wide).toBe(false);
});

test("a signed-in admin's edit on the main page carries the session's token", async ({ page }) => {
  await signIn(page);
  const me = await page.evaluate(() => fetch('/api/auth/me').then((r) => r.json()));
  let token: string | undefined;
  await page.route('**/api/calls/*/transcript', async (route) => {
    token = route.request().headers()['x-csrf-token'];
    const id = Number(route.request().url().match(/calls\/(\d+)/)![1]);
    await route.fulfill({ json: { id, transcript: 'Saved by an admin', origin: 'human' } });
  });
  await page.goto('/');
  const tr = page.locator('tbody tr').nth(2);
  await tr.locator('details.row-actions summary').click();
  await tr.getByRole('button', { name: 'Edit text' }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit the text' });
  await dialog.getByLabel('Text').fill('Saved by an admin');
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(dialog).toBeHidden();
  expect(token).toBe(me.csrf);
});

for (const [theme, width] of [['light', 1280], ['dark', 360]] as const) {
  test(`no accessibility fault signing in or on the status in the ${theme} theme at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.addInitScript((t) => localStorage.setItem('scannerpulse.theme', t), theme);
    const axe = async () => {
      const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
      expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
    };
    await page.goto('/admin');
    await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
    await axe();
    await signIn(page);
    await expect(page.getByRole('heading', { name: 'Newest calls' })).toBeVisible();
    await axe();
  });
}
