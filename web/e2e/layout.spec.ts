import { expect, test } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The "Layout" area. The test host is one machine alone: no plan, no
// database machines; its web service says it is up within seconds.

test.beforeEach(async ({ page }) => freshStorage(page));

test('the layout of one machine shows it answering and no failover', async ({ page }) => {
  await signIn(page, '#layout');
  await expect(page.getByRole('heading', { name: 'Layout', level: 1 })).toBeVisible();
  await expect(page.getByText('One machine: no failover.')).toBeVisible();
  await expect(page.getByText('Read at', { exact: false })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'ScannerPulse machines' })).toBeVisible();
  await expect(page.getByText('answering (version', { exact: false })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Switchover' })).toHaveCount(0);
});

test('one machine with no plan is told how to plan its conversion', async ({ page }) => {
  await signIn(page, '#layout');
  await expect(page.getByText('To convert this install to high availability, run', { exact: false })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Conversion to high availability' })).toHaveCount(0);
});

test("the conversion's guide opens the step to take now, and asks the root helper after a confirmation", async ({
  page,
}) => {
  let asked: unknown = null;
  const step = (id: string, title: string, state: string, extra = {}) => ({
    id,
    title,
    text: `What ${id} does.`,
    on: null,
    run: [],
    ask: null,
    state,
    ...extra,
  });
  await page.route('**/api/admin/convert', (route) =>
    route.fulfill({
      json: {
        plan: { nodes: ['db1.example.org', 'db2.example.org'], hub: '192.0.2.10', referee: null, second: null, shared: null, archive: null },
        guide: {
          steps: [
            step('db-open', "Open this machine's database to db1.example.org and db2.example.org", 'done'),
            step('db1-join', 'Join db1.example.org, the first database machine', 'done', {
              on: 'db1.example.org',
              run: ['curl -fsSL https://example.org/install.sh | sh'],
            }),
            step('db-switch', 'Switch the database to db1.example.org', 'now', {
              ask: {
                kind: 'db_switch',
                label: 'Switch the database to db1.example.org',
                question: 'Make db1.example.org the primary now? Writes pause for a few seconds.',
              },
            }),
            step('db2-join', 'Join db2.example.org, the second database machine', 'later'),
          ],
          checks: [],
          complete: false,
          back: 'Until the switch, `sudo /usr/local/bin/scannerpulse db move --cancel` on this machine takes the site back to its own database alone.',
        },
      },
    }),
  );
  await page.route('**/api/admin/updates', (route) =>
    route.fulfill({
      json: {
        running: '0.13.0',
        machines: [],
        newest: null,
        check: true,
        helper: { host: 'radio-a', version: '0.13.0', seen_seconds_ago: 2, up: true },
        requests: {},
        open: false,
      },
    }),
  );
  await page.route('**/api/admin/helper', (route) => {
    asked = route.request().postDataJSON();
    return route.fulfill({ status: 202, json: { id: 9 } });
  });
  await signIn(page, '#layout');
  await expect(page.getByRole('heading', { name: 'Conversion to high availability' })).toBeVisible();
  await expect(page.getByText('Step 3 of 4: Switch the database to db1.example.org.')).toBeVisible();
  // Only the step to take now says what it does; a done one's commands stay closed.
  await expect(page.getByText('What db-switch does.')).toBeVisible();
  await expect(page.getByText('What db1-join does.')).toHaveCount(0);
  // Until the switch, how to go back.
  await expect(page.getByText('db move --cancel', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Switch the database to db1.example.org' }).click();
  await expect(page.getByText('Make db1.example.org the primary now?', { exact: false })).toBeVisible();
  expect(asked).toBeNull();
  await page.getByRole('group', { name: 'Switch the database to db1.example.org' }).getByRole('button', { name: 'Switch the database to db1.example.org' }).click();
  await expect.poll(() => asked).toEqual({ kind: 'db_switch' });
});
