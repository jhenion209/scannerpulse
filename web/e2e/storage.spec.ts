import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Storage area. The locations and the move are the server's answers
// faked here, so that no folder on the server's machine is touched; a
// refused folder is the real server's.

test.beforeEach(async ({ page }) => freshStorage(page));

const json = (body: unknown, status = 200) => ({ status, contentType: 'application/json', body: JSON.stringify(body) });

const now = () => new Date().toISOString();

const STORAGE = () => ({
  locations: [
    {
      id: 1, name: 'Main archive', root: '/srv/archive', mode: 'managed', primary: true,
      hosts: [{ host: 'host-a', ok: true, problem: null, free_bytes: 400_000_000_000, total_bytes: 2_500_000_000_000, checked_at: now() }],
      calls: 9_812_440, first_call: '2025-01-01T06:00:00Z', last_call: now(), bytes_about: 2_100_000_000_000,
    },
    {
      id: 2, name: 'Cold 2024', root: '/srv/cold-2024', mode: 'managed', primary: false,
      hosts: [
        { host: 'host-a', ok: true, problem: null, free_bytes: 1_100_000_000_000, total_bytes: 2_000_000_000_000, checked_at: now() },
        { host: 'host-b', ok: false, problem: '/srv/cold-2024 cannot be listed: Permission denied', free_bytes: null, total_bytes: null, checked_at: now() },
      ],
      calls: 3_204_118, first_call: '2024-01-01T06:00:00Z', last_call: '2024-12-31T23:00:00Z', bytes_about: 900_000_000_000,
    },
  ],
  hosts: ['host-a', 'host-b'],
  counted_at: now(),
});

const PREVIEW = {
  to: 'Cold 2024', to_id: 2, from_day: '2025-01-01', to_day: '2025-06-30', calls: 412_355,
  held: [{ location: 'Main archive', calls: 412_355 }], bytes_about: 96_000_000_000, free_bytes: 1_100_000_000_000, refused: null,
};

/** Fakes the area's reads; returns the bodies of the moves started. */
async function fake(page: Page, preview: unknown = PREVIEW) {
  const started: unknown[] = [];
  await page.route('**/api/admin/storage', (route) =>
    route.request().method() === 'GET' ? route.fulfill(json(STORAGE())) : route.continue(),
  );
  await page.route('**/api/admin/storage/move**', (route) => {
    if (route.request().method() === 'GET') return route.fulfill(json(preview));
    started.push(route.request().postDataJSON());
    return route.fulfill(json({ job: 31, calls: 412_355 }, 201));
  });
  return started;
}

const card = (page: Page, name: string) =>
  page.getByRole('listitem').filter({ has: page.getByRole('heading', { name: new RegExp(`^${name}`) }) });

test('each location shows its calls, its space and the machines that can use it', async ({ page }) => {
  await fake(page);
  await signIn(page, '#storage');
  await expect(page.getByRole('heading', { name: 'Storage', level: 1 })).toBeVisible();
  await expect(card(page, 'Main archive').getByText('new calls are filed here')).toBeVisible();
  await expect(card(page, 'Cold 2024').getByText('Calls from 2024-01-01 to 2024-12-31 · 3,204,118 calls · about 900.0 GB used')).toBeVisible();
  await expect(card(page, 'Cold 2024').getByText('1.1 TB free')).toBeVisible();
  await expect(card(page, 'Cold 2024').getByText('host-b ✗ cannot use it: /srv/cold-2024 cannot be listed: Permission denied')).toBeVisible();
  const scan = await new AxeBuilder({ page }).include('main').analyze();
  expect(scan.violations).toEqual([]);
});

test('a move is counted, its word typed, and confirmed in a pop-up whose Cancel has the focus', async ({ page }) => {
  const started = await fake(page);
  await signIn(page, '#storage');
  await page.getByRole('button', { name: 'Move a date range…' }).click();
  await page.getByLabel('From').fill('2025-01-01');
  await page.getByLabel('To', { exact: true }).fill('2025-06-30');
  await page.getByLabel('Into').selectOption('Cold 2024');
  await page.getByRole('button', { name: 'Count first' }).click();
  await expect(page.getByText('412,355 calls (about 96.0 GB) of 2025-01-01 to 2025-06-30 would move into Cold 2024')).toBeVisible();
  const last = page.getByRole('button', { name: 'Move 412,355 calls to Cold 2024' });
  await expect(last).toBeDisabled();
  await page.getByLabel('Type move to confirm').fill('Move');
  await expect(last).toBeDisabled();
  await page.getByLabel('Type move to confirm').fill('move');
  await last.click();
  const ask = page.getByRole('dialog', { name: 'Move 412,355 calls to Cold 2024?' });
  await expect(ask).toBeVisible();
  await expect(ask.getByText('There is no undo.')).toBeVisible();
  await expect(ask.getByRole('button', { name: 'Cancel' })).toBeFocused();
  const scan = await new AxeBuilder({ page }).analyze();
  expect(scan.violations).toEqual([]);
  // Enter cancels.
  await page.keyboard.press('Enter');
  await expect(ask).toBeHidden();
  expect(started).toEqual([]);
  await last.click();
  await ask.getByRole('button', { name: 'Move 412,355 calls' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Job 31 moves 412,355 calls into Cold 2024' })).toBeVisible();
  expect(started).toEqual([{ to: 'Cold 2024', from: '2025-01-01', until: '2025-06-30', confirm: 'move' }]);
});

test('a move that cannot start says why, and asks for no word', async ({ page }) => {
  await fake(page, { ...PREVIEW, refused: 'host-b cannot use Cold 2024: /srv/cold-2024 cannot be listed: Permission denied' });
  await signIn(page, '#storage');
  await page.getByRole('button', { name: 'Move a date range…' }).click();
  await page.getByLabel('From').fill('2025-01-01');
  await page.getByLabel('To', { exact: true }).fill('2025-06-30');
  await page.getByLabel('Into').selectOption('Cold 2024');
  await page.getByRole('button', { name: 'Count first' }).click();
  await expect(page.getByRole('alert')).toHaveText(
    'It cannot start: host-b cannot use Cold 2024: /srv/cold-2024 cannot be listed: Permission denied',
  );
  await expect(page.getByLabel('Type move to confirm')).toHaveCount(0);
});

test("a folder that cannot be used is refused in the server's words", async ({ page }) => {
  await signIn(page, '#storage');
  await page.getByRole('button', { name: 'Add a storage path…' }).click();
  await page.getByLabel('Name').fill(`west-${Date.now().toString(36)}`);
  await page.getByLabel('Folder').fill('/no/such/folder');
  await page.getByRole('button', { name: 'Add the location' }).click();
  await expect(page.getByRole('alert')).toHaveText(
    'Not added: /no/such/folder does not exist on this machine: mount the disk or share there first',
  );
});

const PURGE = {
  before: '2025-03-02', calls: 402_117,
  held: [
    { location: 'Main archive', read_only: false, calls: 400_000, bytes_about: 90_000_000_000 },
    { location: 'Old', read_only: true, calls: 2_117, bytes_about: null },
  ],
  bytes_about: 90_000_000_000, refused: null,
};

test('a purge stands apart in red, is counted, its day typed, and confirmed in a pop-up whose Cancel has the focus', async ({ page }) => {
  await fake(page);
  const started: unknown[] = [];
  await page.route('**/api/admin/storage/purge**', (route) => {
    if (route.request().method() === 'GET') return route.fulfill(json(PURGE));
    started.push(route.request().postDataJSON());
    return route.fulfill(json({ job: 32, calls: 402_117 }, 201));
  });
  await signIn(page, '#storage');
  const purge = page.getByRole('button', { name: 'Purge older than…' });
  const move = page.getByRole('button', { name: 'Move a date range…' });
  // Far to the right of the others.
  const [p, m] = [await purge.boundingBox(), await move.boundingBox()];
  expect(p!.x).toBeGreaterThan(m!.x + m!.width + 40);
  await purge.click();
  await page.getByLabel('Before').fill('2025-03-02');
  await page.getByRole('button', { name: 'Count first' }).click();
  await expect(page.getByText(
    '402,117 calls before 2025-03-02 (about 90.0 GB) would be deleted for good: 400,000 in Main archive, 2,117 in Old (read-only: only their records go).',
  )).toBeVisible();
  const last = page.getByRole('button', { name: 'Delete 402,117 calls' });
  await expect(last).toBeDisabled();
  await page.getByLabel('Type 2025-03-02 to confirm').fill('2025-03-01');
  await expect(last).toBeDisabled();
  await page.getByLabel('Type 2025-03-02 to confirm').fill('2025-03-02');
  await last.click();
  const ask = page.getByRole('dialog', { name: 'Delete 402,117 calls before 2025-03-02?' });
  await expect(ask).toBeVisible();
  await expect(ask.getByText('There is no undo.')).toBeVisible();
  await expect(ask.getByRole('button', { name: 'Cancel' })).toBeFocused();
  const scan = await new AxeBuilder({ page }).analyze();
  expect(scan.violations).toEqual([]);
  // Enter cancels.
  await page.keyboard.press('Enter');
  await expect(ask).toBeHidden();
  expect(started).toEqual([]);
  await last.click();
  await ask.getByRole('button', { name: 'Delete 402,117 calls' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Job 32 deletes 402,117 calls before 2025-03-02' })).toBeVisible();
  expect(started).toEqual([{ before: '2025-03-02', confirm: '2025-03-02' }]);
});
