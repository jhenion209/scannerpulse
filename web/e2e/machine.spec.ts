import { expect, test } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The "This machine" area. The test host serves plain HTTP on a loopback
// address and has no settings file, so no backups: its database is not one
// setup installed.

test.beforeEach(async ({ page }) => freshStorage(page));

test('this machine says what it serves and how each part is changed', async ({ page }) => {
  await signIn(page, '#machine');
  await expect(page.getByRole('heading', { name: 'This machine', level: 1 })).toBeVisible();
  await expect(page.getByText("The site serves plain HTTP on this machine's own address only.")).toBeVisible();
  await expect(page.getByText('The database is one you run: its backups are yours.')).toBeVisible();
  await expect(page.getByText('sudo /usr/local/bin/scannerpulse tls install', { exact: false })).toBeVisible();
});
