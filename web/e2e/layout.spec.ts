import { expect, test } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The "Layout" area. The test host is one machine alone: no plan, no
// database machines; its web service says it is up within seconds.

test.beforeEach(async ({ page }) => freshStorage(page));

test('the layout of one machine shows it answering and no failover', async ({ page }) => {
  await signIn(page, '#layout');
  await expect(page.getByRole('heading', { name: 'Layout', level: 1 })).toBeVisible();
  await expect(page.getByText('One machine: no failover.')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'ScannerPulse machines' })).toBeVisible();
  await expect(page.getByText('answering (version', { exact: false })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Switchover' })).toHaveCount(0);
});
