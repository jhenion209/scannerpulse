import { expect, test } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The "This machine" area. The test host serves plain HTTP on a loopback
// address, its database is on the same machine, and it keeps no backups.

test.beforeEach(async ({ page }) => freshStorage(page));

test('this machine says what it serves and how each part is changed', async ({ page }) => {
  await signIn(page, '#machine');
  await expect(page.getByRole('heading', { name: 'This machine', level: 1 })).toBeVisible();
  await expect(page.getByText("The site serves plain HTTP on this machine's own address only.")).toBeVisible();
  await expect(page.getByText('No backups are set up on this machine.')).toBeVisible();
  await expect(page.getByText('sudo /usr/local/bin/scannerpulse tls install', { exact: false })).toBeVisible();
  await expect(page.getByText('sudo /usr/local/bin/scannerpulse backups location', { exact: false }).first()).toBeVisible();
});
