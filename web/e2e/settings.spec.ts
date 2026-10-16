import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Settings area. The one setting changed here, web.system_order, is
// removed again after each test.

test.beforeEach(async ({ page }) => freshStorage(page));
test.afterEach(async ({ page }) => {
  await page.evaluate(async () => {
    const me = await fetch('/api/auth/me');
    if (!me.ok) return;
    const { csrf } = await me.json();
    await fetch('/api/admin/settings/web.system_order', { method: 'DELETE', headers: { 'x-csrf-token': csrf } });
  });
});

const field = (page: Page, key: string) => page.locator('form.setting').filter({ has: page.getByText(key, { exact: true }) });

test('every setting is listed, stored or not', async ({ page }) => {
  await signIn(page, '#settings');
  await expect(field(page, 'oidc.issuer')).toContainText('Not set');
  await expect(field(page, 'transcription.servers')).toBeVisible();
  await expect(field(page, 'time_zone')).toBeVisible();
});

test("a value is saved through the server's checks, kept, and removed", async ({ page }) => {
  await signIn(page, '#settings');
  const order = field(page, 'web.system_order');
  await order.getByRole('textbox').fill('["Valley", "Metro"]');
  await order.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Saved web.system_order.' })).toBeVisible();
  await page.reload();
  await expect(field(page, 'web.system_order').getByRole('textbox')).toHaveValue('["Valley","Metro"]');
  await expect(field(page, 'web.system_order')).toContainText('Changed');
  await field(page, 'web.system_order').getByRole('button', { name: 'Remove' }).click();
  await expect(field(page, 'web.system_order')).toContainText('Not set');
  await expect(field(page, 'web.system_order').getByRole('textbox')).toHaveValue('');
});

test("a refused value is shown beside its field, in the server's words", async ({ page }) => {
  await signIn(page, '#settings');
  const switchField = field(page, 'maintenance.enabled');
  await switchField.getByRole('textbox').fill('maybe');
  await switchField.getByRole('button', { name: 'Save' }).click();
  await expect(switchField.getByRole('alert')).toContainText('is not true or false');
  await switchField.getByRole('textbox').fill('');
  await switchField.getByRole('button', { name: 'Save' }).click();
  await expect(switchField.getByRole('alert')).toHaveText('Not saved: Type a value first, or remove the setting.');
});
