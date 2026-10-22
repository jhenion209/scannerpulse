import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Data area, against e2e/seed.sql. Nothing here changes the seed: a
// rename that would is answered by the test, and so is every job start,
// since nothing runs jobs here and the Jobs tests count the seeded ones.

test.beforeEach(async ({ page }) => freshStorage(page));

const json = (body: unknown, status = 200) => ({ status, contentType: 'application/json', body: JSON.stringify(body) });
/** Answers the POSTs to a path; the GETs go to the server. */
async function answerPosts(page: Page, path: string, body: unknown, status = 201) {
  const asked: unknown[] = [];
  await page.route(`**${path}`, (route) => {
    if (route.request().method() !== 'POST') return route.fallback();
    asked.push(route.request().postDataJSON());
    return route.fulfill(json(body, status));
  });
  return asked;
}

test('a rename is saved, and says when the files keep the old name', async ({ page }) => {
  const asked = await answerPosts(page, '/api/admin/talkgroups/*/rename', { id: 1, name: 'Metro Fire North', files_correct: false }, 200);
  await signIn(page, '#data');
  await page.getByLabel('Talkgroup to rename').selectOption({ label: 'Metro Fire Dispatch · Metro 101' });
  await expect(page.getByLabel('New name')).toHaveValue('Metro Fire Dispatch');
  await page.getByLabel('New name').fill('Metro Fire North');
  await page.getByRole('button', { name: 'Rename' }).click();
  await expect(page.getByRole('status').filter({ hasText: "Renamed to Metro Fire North. Its calls' files keep the old name until they are corrected." })).toBeVisible();
  expect(asked).toEqual([{ name: 'Metro Fire North' }]);
});

test('a double click on Rename asks once', async ({ page }) => {
  let posts = 0;
  await page.route('**/api/admin/talkgroups/*/rename', async (route) => {
    posts += 1;
    await new Promise((resolve) => setTimeout(resolve, 300));
    await route.fulfill(json({ id: 1, name: 'Metro Fire North', files_correct: true }));
  });
  await signIn(page, '#data');
  await page.getByLabel('Talkgroup to rename').selectOption({ label: 'Metro Fire Dispatch · Metro 101' });
  await page.getByLabel('New name').fill('Metro Fire North');
  await page.getByRole('button', { name: 'Rename' }).dblclick();
  await expect(page.getByRole('status').filter({ hasText: 'Renamed to Metro Fire North.' })).toBeVisible();
  expect(posts).toBe(1);
});

test('a name already taken is refused, and the merge into that talkgroup is offered', async ({ page }) => {
  const asked = await answerPosts(page, '/api/admin/talkgroups/*/merge', { id: 11, count: 12 });
  await signIn(page, '#data');
  await page.getByLabel('Talkgroup to rename').selectOption({ label: 'Metro Police One · Metro 102' });
  await page.getByLabel('New name').fill('Metro Fire Dispatch');
  await page.getByRole('button', { name: 'Rename' }).click();
  await expect(page.getByRole('alert')).toHaveText(/^Not renamed: talkgroup \d+ \(Metro\/101\) has that name already: merge into it instead$/);
  await page.getByRole('button', { name: 'Merge into it' }).click();
  await expect(page.getByLabel('Talkgroup that goes')).toBeFocused();
  await expect(page.getByLabel('Talkgroup that stays').locator('option:checked')).toHaveText('Metro Fire Dispatch · Metro 101');
  await expect(page.getByRole('status').filter({ hasText: 'calls of Metro Police One' }))
    .toHaveText(/^\d+ calls of Metro Police One \(Metro\/102\) go to Metro Fire Dispatch \(Metro\/101\); Metro Police One goes\.$/);
  const merge = page.getByRole('button', { name: 'Merge', exact: true });
  await page.getByLabel('Type Metro Police One to confirm').fill('metro police one');
  await expect(merge).toBeDisabled();
  await page.getByLabel('Type Metro Police One to confirm').fill('Metro Police One');
  await merge.click();
  await expect(page.getByRole('status').filter({ hasText: 'Job #11 merges 12 calls; follow it in Jobs.' })).toBeVisible();
  await expect(page.getByLabel('Talkgroup that goes')).toBeFocused();
  expect(asked).toHaveLength(1);
  expect(asked[0]).toMatchObject({ confirm: 'Metro Police One' });
});

test("the talkgroup that stays is one of the same system's", async ({ page }) => {
  await signIn(page, '#data');
  await page.getByLabel('Talkgroup that goes').selectOption({ label: 'Metro Fire Dispatch · Metro 101' });
  await expect(page.getByLabel('Talkgroup that stays').locator('option')).toHaveText(['Pick one', 'Metro Police One · Metro 102']);
  await page.getByLabel('Talkgroup that goes').selectOption({ label: 'Valley Ambulance · Valley 201' });
  await expect(page.getByLabel('Talkgroup that stays').locator('option')).toHaveText(['Pick one']);
});

test('a count that comes back after a field changed is dropped', async ({ page }) => {
  await signIn(page, '#data');
  let release!: () => void;
  const held = new Promise<void>((resolve) => (release = resolve));
  await page.route('**/api/admin/talkgroups/*/merge?*', async (route) => {
    await held;
    await route.fulfill(json({
      from: { id: 2, system: 'Metro', number: 102, name: 'Metro Police One' },
      into: { id: 1, system: 'Metro', number: 101, name: 'Metro Fire Dispatch' },
      calls: 12,
    }));
  });
  await page.getByLabel('Talkgroup that goes').selectOption({ label: 'Metro Police One · Metro 102' });
  await page.getByLabel('Talkgroup that stays').selectOption({ label: 'Metro Fire Dispatch · Metro 101' });
  const counting = page.waitForRequest((r) => r.url().includes('/merge?into='));
  await page.getByRole('button', { name: 'Count the calls to move' }).click();
  await counting;
  await page.getByLabel('Talkgroup that stays').selectOption({ label: 'Pick one' });
  const answered = page.waitForResponse((r) => r.url().includes('/merge?into='));
  release();
  await (await answered).finished();
  // The page has had time to use the answer it should have dropped.
  await page.waitForTimeout(100);
  await expect(page.getByRole('status').filter({ hasText: 'calls of' })).toHaveCount(0);
  await expect(page.getByLabel(/^Type .* to confirm$/)).toHaveCount(0);
});

test("with every talkgroup's files correct, the list says so", async ({ page }) => {
  await signIn(page, '#data');
  await expect(page.getByText("Every talkgroup's files carry its name.")).toBeVisible();
});

test("the buttons of two talkgroups with one name are told apart by their system and number", async ({ page }) => {
  await page.route('**/api/admin/data/uncorrected', (route) =>
    route.fulfill(json([
      { id: 1, system: 'Metro', number: 101, name: 'Dispatch', files_name: null, calls: 4 },
      { id: 3, system: 'Valley', number: 201, name: 'Dispatch', files_name: null, calls: 5 },
    ])));
  await signIn(page, '#data');
  await expect(page.getByRole('button', { name: 'Correct the files of Dispatch (Metro 101)', exact: true })).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Correct the files of Dispatch (Valley 201)', exact: true })).toHaveCount(1);
});

test('a failed read says which list it is, and the other list still shows', async ({ page }) => {
  await page.route('**/api/admin/data/uncorrected', (route) => route.fulfill(json({ error: 'service unavailable', detail: 'the database does not answer' }, 503)));
  await signIn(page, '#data');
  await expect(page.getByRole('alert')).toHaveText('The files not yet corrected could not be read: the database does not answer');
  await expect(page.getByText('Reading…')).toHaveCount(0);
  await expect(page.getByLabel('Talkgroup to rename').locator('option')).toHaveCount(4);
  await page.unroute('**/api/admin/data/uncorrected');

  await page.route('**/api/admin/talkgroups', (route) => route.fulfill(json({ error: 'service unavailable', detail: 'the database does not answer' }, 503)));
  await page.reload();
  await expect(page.getByRole('alert')).toHaveText('The talkgroups could not be read: the database does not answer');
  await expect(page.getByLabel('Talkgroup to rename').locator('option')).toHaveText(['Pick one']);
  await expect(page.getByText("Every talkgroup's files carry its name.")).toBeVisible();
});

/** The talkgroup list the server gives, from its second read on without Metro Fire Dispatch:
 * another admin has merged it away. Counts the reads. */
async function dispatchGoneAfterFirstRead(page: Page) {
  const seen = { reads: 0 };
  await page.route('**/api/admin/talkgroups', async (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    seen.reads += 1;
    if (seen.reads === 1) return route.fallback();
    const response = await route.fetch();
    const all: { name: string }[] = await response.json();
    return route.fulfill({ response, json: all.filter((t) => t.name !== 'Metro Fire Dispatch') });
  });
  return seen;
}

test('a rename refused because its talkgroup is gone reads the list again, and the refusal stays', async ({ page }) => {
  const seen = await dispatchGoneAfterFirstRead(page);
  await page.route('**/api/admin/talkgroups/*/rename', (route) =>
    route.fulfill(json({ error: 'not found', detail: 'there is no talkgroup 1' }, 404)));
  await signIn(page, '#data');
  await page.getByLabel('Talkgroup to rename').selectOption({ label: 'Metro Fire Dispatch · Metro 101' });
  await page.getByLabel('New name').fill('Metro Fire North');
  await page.getByRole('button', { name: 'Rename' }).click();
  await expect(page.getByRole('alert')).toHaveText('Not renamed: there is no talkgroup 1');
  await expect(page.getByLabel('Talkgroup to rename').locator('option')).toHaveText(['Pick one', 'Metro Police One · Metro 102', 'Valley Ambulance · Valley 201']);
  expect(seen.reads).toBe(2);
});

test('a merge count refused because a talkgroup is gone reads the list again, and the refusal stays', async ({ page }) => {
  const seen = await dispatchGoneAfterFirstRead(page);
  await page.route('**/api/admin/talkgroups/*/merge?*', (route) =>
    route.fulfill(json({ error: 'not found', detail: 'there is no talkgroup 1' }, 404)));
  await signIn(page, '#data');
  await page.getByLabel('Talkgroup that goes').selectOption({ label: 'Metro Police One · Metro 102' });
  await page.getByLabel('Talkgroup that stays').selectOption({ label: 'Metro Fire Dispatch · Metro 101' });
  await page.getByRole('button', { name: 'Count the calls to move' }).click();
  // The list no longer offers it, so the select is emptied; its refusal must not go with it.
  await expect(page.getByLabel('Talkgroup that stays').locator('option')).toHaveText(['Pick one']);
  await expect(page.getByRole('alert')).toHaveText('Not merged: there is no talkgroup 1');
  expect(seen.reads).toBe(2);
});

test('a purge refused because its talkgroup is gone reads the list again, and the refusal stays', async ({ page }) => {
  const seen = await dispatchGoneAfterFirstRead(page);
  await page.route('**/api/admin/talkgroups/*/purge', (route) =>
    route.fulfill(json({ error: 'not found', detail: 'there is no talkgroup 1' }, 404)));
  await signIn(page, '#data');
  await page.getByLabel('Talkgroup to purge').selectOption({ label: 'Metro Fire Dispatch · Metro 101' });
  await page.getByRole('button', { name: 'Count the calls to delete' }).click();
  await expect(page.getByRole('alert')).toHaveText('Not purged: there is no talkgroup 1');
  await expect(page.getByLabel('Talkgroup to purge').locator('option')).toHaveText(['Pick one', 'Metro Police One · Metro 102', 'Valley Ambulance · Valley 201']);
  expect(seen.reads).toBe(2);
});

test('a started job reads the lists again', async ({ page }) => {
  const valley = { id: 3, system: 'Valley', number: 201, name: 'Valley Ambulance' };
  await page.route('**/api/admin/talkgroups/*/purge', (route) => {
    if (route.request().method() === 'GET') return route.fulfill(json({ talkgroup: valley, calls: 1, read_only: 0, holding_path: '/srv/holding' }));
    return route.fulfill(json({ id: 13, count: 1 }, 201));
  });
  let reads = { talkgroups: 0, uncorrected: 0 };
  page.on('request', (r) => {
    if (r.method() !== 'GET') return;
    if (r.url().endsWith('/api/admin/talkgroups')) reads.talkgroups += 1;
    if (r.url().endsWith('/api/admin/data/uncorrected')) reads.uncorrected += 1;
  });
  await signIn(page, '#data');
  await page.getByLabel('Talkgroup to purge').selectOption({ label: 'Valley Ambulance · Valley 201' });
  await page.getByRole('button', { name: 'Count the calls to delete' }).click();
  // One call reads in the singular.
  await expect(page.getByRole('status').filter({ hasText: 'call of Valley Ambulance' }))
    .toHaveText('1 call of Valley Ambulance (Valley/201) is deleted; its file goes to /srv/holding.');
  await page.getByLabel('Type Valley Ambulance to confirm').fill('Valley Ambulance');
  await page.getByRole('button', { name: 'Purge', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Job #13 purges 1 call; follow it in Jobs.' })).toBeVisible();
  await expect.poll(() => reads).toEqual({ talkgroups: 2, uncorrected: 2 });
});

test("a talkgroup's files are corrected after their count and its name typed", async ({ page }) => {
  const metro = { id: 1, system: 'Metro', number: 101, name: 'Metro Fire North' };
  await page.route('**/api/admin/data/uncorrected', (route) =>
    route.fulfill(json([{ ...metro, files_name: 'Metro Fire Dispatch', calls: 12 }])));
  const asked: unknown[] = [];
  await page.route('**/api/admin/talkgroups/1/correct', (route) => {
    if (route.request().method() === 'GET') return route.fulfill(json({ talkgroup: metro, calls: 10, read_only: 2 }));
    asked.push(route.request().postDataJSON());
    return route.fulfill(json({ id: 12, count: 10 }, 201));
  });
  await signIn(page, '#data');
  const row = page.getByRole('table', { name: 'Talkgroups whose files are not yet corrected' }).locator('tbody tr');
  await expect(row).toHaveText(/Metro Fire North\s*Metro 101\s*Metro Fire Dispatch\s*12/);
  await row.getByRole('button', { name: 'Correct the files of Metro Fire North (Metro 101)', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'files of Metro Fire North' }))
    .toHaveText("10 files of Metro Fire North (Metro/101) move to its name's folder; 2 in read-only locations stay.");
  const typed = page.getByLabel('Type Metro Fire North to confirm');
  // A phone's forced capital would make the exact name unwinnable.
  await expect(typed).toHaveAttribute('autocapitalize', 'off');
  await expect(typed).toHaveAttribute('autocorrect', 'off');
  await typed.fill('Metro Fire North');
  await page.getByRole('button', { name: 'Correct the files', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Job #12 corrects 10 files; follow it in Jobs.' })).toBeVisible();
  expect(asked).toEqual([{ confirm: 'Metro Fire North' }]);
  await expect(page.getByRole('button', { name: 'Correct the files of Metro Fire North (Metro 101)', exact: true })).toBeFocused();
});

test("another row's count replaces one that is still coming", async ({ page }) => {
  const north = { id: 1, system: 'Metro', number: 101, name: 'Metro Fire North' };
  const south = { id: 2, system: 'Metro', number: 102, name: 'Metro Fire South' };
  await page.route('**/api/admin/data/uncorrected', (route) =>
    route.fulfill(json([{ ...north, files_name: null, calls: 12 }, { ...south, files_name: null, calls: 7 }])));
  let release!: () => void;
  const held = new Promise<void>((resolve) => (release = resolve));
  await page.route('**/api/admin/talkgroups/1/correct', async (route) => {
    await held;
    await route.fulfill(json({ talkgroup: north, calls: 12, read_only: 0 }));
  });
  await page.route('**/api/admin/talkgroups/2/correct', (route) =>
    route.fulfill(json({ talkgroup: south, calls: 7, read_only: 0 })));
  await signIn(page, '#data');
  const counting = page.waitForRequest((r) => r.url().endsWith('/talkgroups/1/correct'));
  await page.getByRole('button', { name: 'Correct the files of Metro Fire North' }).click();
  await counting;
  await page.getByRole('button', { name: 'Correct the files of Metro Fire South' }).click();
  await expect(page.getByLabel('Type Metro Fire South to confirm')).toBeVisible();
  const answered = page.waitForResponse((r) => r.url().endsWith('/talkgroups/1/correct'));
  release();
  await (await answered).finished();
  await page.waitForTimeout(100);
  await expect(page.getByRole('status').filter({ hasText: 'files of' })).toHaveText(/^7 files of Metro Fire South/);
  await expect(page.getByLabel('Type Metro Fire North to confirm')).toHaveCount(0);
});

test("a purge shows the holding folder's rule, and waits for the setting", async ({ page }) => {
  await signIn(page, '#data');
  await expect(page.getByText('.scannerpulse-holding', { exact: true })).toBeVisible();
  await page.getByLabel('Talkgroup to purge').selectOption({ label: 'Valley Ambulance · Valley 201' });
  await page.getByRole('button', { name: 'Count the calls to delete' }).click();
  await expect(page.getByText("Set purge.holding_path in Settings first: the purged calls' files are kept there.")).toBeVisible();
  await expect(page.getByLabel('Type Valley Ambulance to confirm')).toHaveCount(0);
});

test('a purge starts after its count, the holding folder and its name typed', async ({ page }) => {
  const valley = { id: 3, system: 'Valley', number: 201, name: 'Valley Ambulance' };
  let counted = 0;
  const asked: unknown[] = [];
  await page.route('**/api/admin/talkgroups/*/purge', (route) => {
    if (route.request().method() === 'GET') {
      counted += 1;
      return route.fulfill(json({ talkgroup: valley, calls: 5, read_only: counted === 1 ? 2 : 0, holding_path: '/srv/holding' }));
    }
    asked.push(route.request().postDataJSON());
    return route.fulfill(json({ id: 13, count: 5 }, 201));
  });
  await signIn(page, '#data');
  await page.getByLabel('Talkgroup to purge').selectOption({ label: 'Valley Ambulance · Valley 201' });
  const count = page.getByRole('button', { name: 'Count the calls to delete' });
  await count.click();
  // A call in a read-only location loses only its record.
  await expect(page.getByRole('status').filter({ hasText: 'calls of Valley Ambulance' }))
    .toHaveText('5 calls of Valley Ambulance (Valley/201) are deleted; 3 files go to /srv/holding, and 2 in read-only locations keep their files.');
  await expect(page.getByLabel('Type Valley Ambulance to confirm')).toBeVisible();
  await count.click();
  await expect(page.getByRole('status').filter({ hasText: 'calls of Valley Ambulance' }))
    .toHaveText('5 calls of Valley Ambulance (Valley/201) are deleted; their files go to /srv/holding.');
  await page.getByLabel('Type Valley Ambulance to confirm').fill('Valley Ambulance');
  await page.getByRole('button', { name: 'Purge', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Job #13 purges 5 calls; follow it in Jobs.' })).toBeVisible();
  expect(asked).toEqual([{ confirm: 'Valley Ambulance' }]);
  await expect(page.getByLabel('Talkgroup to purge')).toBeFocused();
});

test('a purge count that comes back after the talkgroup changed is dropped', async ({ page }) => {
  await signIn(page, '#data');
  let release!: () => void;
  const held = new Promise<void>((resolve) => (release = resolve));
  await page.route('**/api/admin/talkgroups/*/purge', async (route) => {
    await held;
    await route.fulfill(json({
      talkgroup: { id: 3, system: 'Valley', number: 201, name: 'Valley Ambulance' },
      calls: 5, read_only: 0, holding_path: '/srv/holding',
    }));
  });
  await page.getByLabel('Talkgroup to purge').selectOption({ label: 'Valley Ambulance · Valley 201' });
  const counting = page.waitForRequest((r) => r.url().endsWith('/purge'));
  await page.getByRole('button', { name: 'Count the calls to delete' }).click();
  await counting;
  await page.getByLabel('Talkgroup to purge').selectOption({ label: 'Pick one' });
  const answered = page.waitForResponse((r) => r.url().endsWith('/purge'));
  release();
  await (await answered).finished();
  await page.waitForTimeout(100);
  await expect(page.getByRole('status').filter({ hasText: 'calls of' })).toHaveCount(0);
  await expect(page.getByLabel(/^Type .* to confirm$/)).toHaveCount(0);
});

test('the area with its list, a count and a typed name has no accessibility fault and no sideways scroll at 360 px', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.addInitScript(() => localStorage.setItem('scannerpulse.theme', 'dark'));
  const metro = { id: 1, system: 'Metro', number: 101, name: 'Metro Fire Dispatch North Side Units' };
  await page.route('**/api/admin/data/uncorrected', (route) =>
    route.fulfill(json([{ ...metro, files_name: 'Metro Fire Dispatch North Side', calls: 12345 }])));
  await page.route('**/api/admin/talkgroups/1/correct', (route) => route.fulfill(json({ talkgroup: metro, calls: 12000, read_only: 345 })));
  // A holding folder's path may be one long word.
  const valley = { id: 3, system: 'Valley', number: 201, name: 'Valley Ambulance' };
  await page.route('**/api/admin/talkgroups/3/purge', (route) =>
    route.fulfill(json({ talkgroup: valley, calls: 12345, read_only: 0, holding_path: `/srv/holding/${'a'.repeat(100)}` })));
  await signIn(page, '#data');
  await page.getByRole('button', { name: `Correct the files of ${metro.name} (Metro 101)`, exact: true }).click();
  await expect(page.getByLabel(`Type ${metro.name} to confirm`)).toBeVisible();
  await page.getByLabel('Talkgroup to purge').selectOption({ label: 'Valley Ambulance · Valley 201' });
  await page.getByRole('button', { name: 'Count the calls to delete' }).click();
  await expect(page.getByLabel('Type Valley Ambulance to confirm')).toBeVisible();
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
  expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)).toBe(false);
});
