import { expect, test } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The "Updates" area. The test host runs no root helper, so Update now is
// shown only once a newer release is known, and it waits for the helper.

test.beforeEach(async ({ page }) => freshStorage(page));

test('updates say the helper is not running and how to start it', async ({ page }) => {
  await signIn(page, '#updates');
  await expect(page.getByRole('heading', { name: 'Updates', level: 1 })).toBeVisible();
  await expect(page.getByText('Read at', { exact: false })).toBeVisible();
  await expect(page.getByText('agent install', { exact: false })).toBeVisible();
  // No release known yet: nothing to update to.
  await expect(page.getByRole('button', { name: 'Update now' })).toHaveCount(0);
});

test('a newer release offers Update now, which asks first and then shows the helper at work', async ({ page }) => {
  let asked = 0;
  const base = {
    running: '0.9.0',
    machines: [{ host: 'radio-a', version: '0.9.0' }],
    newest: { version: '0.9.1', newer: true, notes: 'What 0.9.1 changes.', checked_at: '2026-10-19T20:00:00Z' },
    check: true,
    helper: { host: 'radio-a', version: '0.9.0', seen_seconds_ago: 2, up: true },
  };
  await page.route('**/api/admin/updates', (route) =>
    route.fulfill({
      json: {
        ...base,
        open: asked > 0,
        requests: {
          switchover: null,
          update:
            asked === 0
              ? null
              : {
                  id: 7,
                  kind: 'update',
                  requested_by: 'admin',
                  requested_at: '2026-10-19T20:01:00Z',
                  started_at: '2026-10-19T20:01:02Z',
                  finished_at: null,
                  host: 'radio-a',
                  ok: null,
                  said: 'Updating db1 (0.9.0 to 0.9.1)...\n',
                },
        },
      },
    }),
  );
  await page.route('**/api/admin/helper', (route) => {
    asked += 1;
    return route.fulfill({ status: 202, json: { id: 7 } });
  });
  await signIn(page, '#updates');
  await expect(page.getByText('Update available: 0.9.1 (this site runs 0.9.0).')).toBeVisible();
  await expect(page.getByText('What 0.9.1 changes.')).toBeVisible();
  await page.getByRole('button', { name: 'Update now' }).click();
  await expect(page.getByText('Update every machine to 0.9.1 now?')).toBeVisible();
  await page.getByRole('group', { name: 'Update now' }).getByRole('button', { name: 'Update now' }).click();
  await expect(page.getByText('The update runs on radio-a.')).toBeVisible();
  await expect(page.getByText('Updating db1 (0.9.0 to 0.9.1)...')).toBeVisible();
  expect(asked).toBe(1);
});
