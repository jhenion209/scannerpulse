import { expect, test, type Page } from '@playwright/test';
import { ASSOCIATE, freshStorage, signIn } from './admin';

// Admin roles. e2e/seed.sh makes e2e-associate, who may only move a date
// range; the tests leave that as it is.

test.beforeEach(async ({ page }) => freshStorage(page));

const card = (page: Page, login: string) => page.getByRole('listitem').filter({ has: page.getByRole('heading', { name: new RegExp(`^${login}( \\(you\\))?$`) }) });

test('an associate sees only the changes turned on for them', async ({ page }) => {
  await signIn(page, '#storage', ASSOCIATE);
  await expect(page.getByRole('banner')).toContainText('e2e-associate · associate');
  await expect(page.getByRole('button', { name: 'Move a date range…' })).toBeVisible();
  for (const name of ['Add a storage path…', 'Import an older archive…', 'Purge older than…']) {
    await expect(page.getByRole('button', { name })).toHaveCount(0);
  }
  await page.goto('/admin#maintenance');
  await expect(page.getByText(/^Maintenance is (on|off)\.$/)).toBeVisible();
  await expect(page.getByRole('button', { name: /^Turn maintenance/ })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Rescan now' })).toHaveCount(0);
  await page.goto('/admin#accounts');
  await expect(card(page, ASSOCIATE.login)).toContainText('associate · signs in with a password · enabled');
  await expect(page.locator('.accounts > li')).toHaveCount(1);
  await expect(page.getByRole('button', { name: `New password for ${ASSOCIATE.login}` })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Add the admin' })).toHaveCount(0);
});

test('a super admin turns a function on for an associate, and off again', async ({ page }) => {
  await signIn(page, '#accounts');
  const row = card(page, ASSOCIATE.login);
  const favorites = row.getByRole('checkbox', { name: `Favorites for ${ASSOCIATE.login}` });
  await expect(row.getByRole('checkbox', { name: `Move for ${ASSOCIATE.login}` })).toBeChecked();
  await expect(favorites).not.toBeChecked();
  await favorites.check();
  await expect(page.getByRole('status').filter({ hasText: `Favorites is on for ${ASSOCIATE.login}.` })).toBeVisible();
  await expect(favorites).toBeChecked();
  await favorites.uncheck();
  await expect(page.getByRole('status').filter({ hasText: `Favorites is off for ${ASSOCIATE.login}.` })).toBeVisible();
  await expect(favorites).not.toBeChecked();
});
