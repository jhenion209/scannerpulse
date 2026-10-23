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
      id: 1, name: 'Main archive', root: '/srv/archive', mode: 'managed', primary: true, offset_minutes: null,
      hosts: [{ host: 'host-a', ok: true, problem: null, free_bytes: 400_000_000_000, total_bytes: 2_500_000_000_000, checked_at: now() }],
      calls: 9_812_440, first_call: '2025-01-01T06:00:00Z', last_call: now(), bytes_about: 2_100_000_000_000,
    },
    {
      id: 2, name: 'Cold 2024', root: '/srv/cold-2024', mode: 'managed', primary: false, offset_minutes: null,
      hosts: [
        { host: 'host-a', ok: true, problem: null, free_bytes: 1_100_000_000_000, total_bytes: 2_000_000_000_000, checked_at: now() },
        { host: 'host-b', ok: false, problem: '/srv/cold-2024 cannot be listed: Permission denied', free_bytes: null, total_bytes: null, checked_at: now() },
      ],
      calls: 3_204_118, first_call: '2024-01-01T06:00:00Z', last_call: '2024-12-31T23:00:00Z', bytes_about: 900_000_000_000,
    },
    {
      id: 3, name: 'Old', root: '/srv/old', mode: 'readonly', primary: false, offset_minutes: 60,
      hosts: [{ host: 'host-a', ok: true, problem: null, free_bytes: 10_000_000_000, total_bytes: 50_000_000_000, checked_at: now() }],
      calls: 2_117, first_call: '2019-03-01T19:00:00Z', last_call: '2019-12-31T23:00:00Z', bytes_about: 300_000_000,
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

const REVIEW = {
  name: 'Old', folder: '/srv/old', files: 4_003, taken: 0, no_date: 3, bad_names: 0, left: 4_000, read: 1_000,
  new: 600, copies: 3, unknown: 2, differ: 0, unreadable: 1, before_purge: 0,
  examples: {
    no_date: ['notes/readme.mp3'], new: ['2019/03/01/20190301_120000_a.mp3'], copies: [], unknown: ['2019/03/02/20190302_090000_e.mp3: the system County'],
    differ: [], unreadable: [],
  },
  copies_at: [0, 0, 3, 390, 0], new_at: [600, 600, 600, 213, 600], suggested: 60, kept_offset: null, done: true,
};

test('an older archive is reviewed first, then imported with the offset the review suggests', async ({ page }) => {
  await fake(page);
  const started: unknown[] = [];
  await page.route('**/api/admin/storage/import/review', (route) => route.fulfill(json({ job: 41 }, 201)));
  await page.route('**/api/admin/storage/import/review/41', (route) => route.fulfill(json(REVIEW)));
  await page.route('**/api/admin/storage/import', (route) => {
    started.push(route.request().postDataJSON());
    return route.fulfill(json({ job: 42, location: 3, source: 9, offset: 60, again: false }, 201));
  });
  await signIn(page, '#storage');
  await page.getByRole('button', { name: 'Import an older archive…' }).click();
  await page.getByLabel('Name').fill('Old');
  await page.getByLabel('Folder').fill('/srv/old');
  await page.getByRole('button', { name: 'Review first' }).click();
  await expect(page.getByText('4,003 mp3 files in /srv/old; 3 with no date and time in their names are skipped.')).toBeVisible();
  await expect(page.getByText('Copies at each offset: -02:00 0, -01:00 0, +00:00 3, +01:00 390, +02:00 0.')).toBeVisible();
  await expect(page.getByText('The files are copies once their times are moved by +01:00')).toBeVisible();
  await expect(page.getByLabel('Offset')).toHaveValue('+01:00');
  await page.getByText('Systems not named yet', { exact: true }).click();
  await expect(page.getByText('2019/03/02/20190302_090000_e.mp3: the system County')).toBeVisible();
  const scan = await new AxeBuilder({ page }).include('main').analyze();
  expect(scan.violations).toEqual([]);
  await page.getByRole('button', { name: 'Import up to about 852 calls into Old' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Job 42 imports /srv/old as Old, its times moved by +01:00' })).toBeVisible();
  expect(started).toEqual([{ name: 'Old', folder: '/srv/old', offset: '+01:00' }]);
});

test("an import's card says it is read-only, and it is consolidated once counted, move typed and confirmed", async ({ page }) => {
  await fake(page);
  const started: unknown[] = [];
  await page.route('**/api/admin/storage/consolidate**', (route) => {
    if (route.request().method() === 'GET')
      return route.fulfill(json({ name: 'Old', calls: 2_117, bytes_about: 300_000_000, free_bytes: 400_000_000_000, refused: null }));
    started.push(route.request().postDataJSON());
    return route.fulfill(json({ job: 43, calls: 2_117 }, 201));
  });
  await signIn(page, '#storage');
  const old = card(page, 'Old');
  await expect(old.getByText('read-only: its files are never changed')).toBeVisible();
  await expect(old.getByText("its file names' times are moved by +01:00")).toBeVisible();
  await old.getByRole('button', { name: 'Consolidate… Old' }).click();
  await expect(page.getByText(
    '2,117 calls of Old (about 300.0 MB) would be copied into the main archive, which has 400.0 GB free. Old’s own files stay as they are.',
  )).toBeVisible();
  const last = page.getByRole('button', { name: 'Copy 2,117 calls into the main archive' });
  await expect(last).toBeDisabled();
  await page.getByLabel('Type move to confirm').fill('move');
  await last.click();
  const ask = page.getByRole('dialog', { name: 'Copy 2,117 calls of Old into the main archive?' });
  await expect(ask.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await ask.getByRole('button', { name: 'Copy 2,117 calls' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Job 43 copies 2,117 calls of Old into the main archive' })).toBeVisible();
  expect(started).toEqual([{ name: 'Old', confirm: 'move' }]);
});
