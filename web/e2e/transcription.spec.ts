import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Transcription area, against e2e/seed.sql, where transcription is off
// (`transcription.enabled` is not set). A switch changed here is put back;
// a queue job is answered by the test, since nothing runs jobs here and the
// Jobs tests count the seeded ones.

test.beforeEach(async ({ page }) => freshStorage(page));

const json = (body: unknown, status = 200) => ({ status, contentType: 'application/json', body: JSON.stringify(body) });
const switchOf = (page: Page, name: RegExp) => page.getByRole('checkbox', { name });

test("each talkgroup's switch turns its transcription on and off, and is kept", async ({ page }) => {
  await signIn(page, '#transcription');
  const name = /^Transcribe Metro Fire Dispatch/;
  await expect(switchOf(page, name)).not.toBeChecked();
  const saved = () => page.waitForResponse((r) => r.url().endsWith('/transcribe') && r.request().method() === 'PUT');
  try {
    await Promise.all([saved(), switchOf(page, name).check()]);
    await page.reload();
    await expect(switchOf(page, name)).toBeChecked();
  } finally {
    // The server is not reseeded between runs: the switch goes back off, whatever failed above.
    await page.reload();
    const on = switchOf(page, name);
    if (await on.isChecked()) await Promise.all([saved(), on.uncheck()]);
  }
  await page.reload();
  await expect(switchOf(page, name)).not.toBeChecked();
});

test('the list finds a talkgroup by name or number', async ({ page }) => {
  await signIn(page, '#transcription');
  const rows = page.getByRole('table', { name: "Each talkgroup's transcription switch" }).locator('tbody tr');
  await expect(rows).toHaveCount(3);
  await page.getByLabel('Find a talkgroup').fill('201');
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText('Valley Ambulance');
});

test("a column's header sorts the list, and a second click turns the order round", async ({ page }) => {
  await signIn(page, '#transcription');
  const table = page.getByRole('table', { name: "Each talkgroup's transcription switch" });
  const first = table.locator('tbody tr').first();
  const number = table.getByRole('columnheader', { name: 'Number' });
  await expect(first).toContainText('Metro Fire Dispatch');
  await expect(table.getByRole('columnheader', { name: 'System' })).toHaveAttribute('aria-sort', 'ascending');
  await expect(number).toHaveAttribute('aria-sort', 'none');
  await number.getByRole('button').click();
  await expect(number).toHaveAttribute('aria-sort', 'ascending');
  await expect(first).toContainText('Metro Fire Dispatch');
  await number.getByRole('button').click();
  await expect(number).toHaveAttribute('aria-sort', 'descending');
  await expect(first).toContainText('Valley Ambulance');
  await page.getByLabel('Find a talkgroup').fill('metro');
  await expect(first).toContainText('Metro Police One');
});

test('a switch whose row moves keeps the focus, for the next key', async ({ page }) => {
  await signIn(page, '#transcription');
  const table = page.getByRole('table', { name: "Each talkgroup's transcription switch" });
  const transcribe = table.getByRole('columnheader', { name: 'Transcribe' }).getByRole('button');
  await transcribe.click();
  await transcribe.click();
  const name = /^Transcribe Metro Fire Dispatch/;
  const saved = () => page.waitForResponse((r) => r.url().endsWith('/transcribe') && r.request().method() === 'PUT');
  try {
    await expect(table.locator('tbody tr').first()).toContainText('Metro Fire Dispatch');
    await switchOf(page, name).focus();
    await Promise.all([saved(), page.keyboard.press('Space')]);
    await expect(table.locator('tbody tr').last()).toContainText('Metro Fire Dispatch');
    await expect(switchOf(page, name)).toBeFocused();
  } finally {
    // The server is not reseeded between runs: the switch goes back off, whatever failed above.
    await page.reload();
    const on = switchOf(page, name);
    if (await on.isChecked()) await Promise.all([saved(), on.uncheck()]);
  }
});

test("a queue job is counted first, refused in the server's words, and asked for only after", async ({ page }) => {
  let reads = 0;
  page.on('request', (r) => { if (r.method() === 'GET' && r.url().endsWith('/api/admin/talkgroups')) reads += 1; });
  await signIn(page, '#transcription');
  // The seed sets neither a time zone nor transcription: the server refuses to count.
  await page.getByRole('button', { name: 'Count the calls' }).click();
  await expect(page.getByRole('alert')).toHaveText(/^Not queued: .+/);
  // A refusal may come of a talkgroup that is gone: the list is read again.
  await expect.poll(() => reads).toBe(2);
  await expect(page.getByRole('alert')).toHaveText(/^Not queued: .+/);
  await expect(page.getByRole('button', { name: /^Queue / })).toHaveCount(0);

  let asked: unknown = null;
  await page.route('**/api/admin/jobs/queue*', async (route) => {
    if (route.request().method() === 'GET') return route.fulfill(json({ count: 42, min_ms: 1000 }));
    asked = route.request().postDataJSON();
    return route.fulfill(json({ id: 7, count: 42 }, 201));
  });
  await page.getByLabel('Talkgroup', { exact: true }).selectOption({ label: 'Valley Ambulance · Valley 201' });
  await page.getByLabel('From').fill('2031-07-01');
  await page.getByRole('button', { name: 'Count the calls' }).click();
  await expect(page.getByText('42 calls would be queued (calls shorter than 1 s are left out).')).toBeVisible();
  // A changed field makes the count stale.
  await page.getByLabel('To').fill('2031-07-02');
  await expect(page.getByRole('button', { name: /^Queue / })).toHaveCount(0);
  await page.getByRole('button', { name: 'Count the calls' }).click();
  await page.getByRole('button', { name: 'Queue 42 calls' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Job #7 queues 42 calls' })).toBeVisible();
  expect(asked).toMatchObject({ from: '2031-07-01', to: '2031-07-02' });
  expect((asked as { talkgroup_id: number }).talkgroup_id).toBeGreaterThan(0);
});

test('the failed calls are put back in the queue', async ({ page }) => {
  await signIn(page, '#transcription');
  await page.getByRole('button', { name: 'Try the failed calls again' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'No failed call to try again.' })).toBeVisible();
});

test('a double click puts the failed calls back once, and one call reads in the singular', async ({ page }) => {
  let posts = 0;
  await page.route('**/api/admin/queue/retry', async (route) => {
    posts += 1;
    await new Promise((resolve) => setTimeout(resolve, 300));
    await route.fulfill(json({ retried: 1 }));
  });
  await signIn(page, '#transcription');
  await page.getByRole('button', { name: 'Try the failed calls again' }).dblclick();
  await expect(page.getByRole('status').filter({ hasText: '1 failed call is back in the queue.' })).toBeVisible();
  expect(posts).toBe(1);
});

test('a refusal to put the failed calls back is an alert, apart from the status', async ({ page }) => {
  await page.route('**/api/admin/queue/retry', (route) => route.fulfill(json({ error: 'service unavailable', detail: 'maintenance is on' }, 503)));
  await signIn(page, '#transcription');
  await page.getByRole('button', { name: 'Try the failed calls again' }).click();
  await expect(page.getByRole('alert')).toHaveText('The failed calls were not put back: maintenance is on');
  await expect(page.getByRole('status').filter({ hasText: 'failed call' })).toHaveCount(0);
});

test('a refused switch goes back to the saved state beside its reason, and the list is read again', async ({ page }) => {
  let reads = 0;
  page.on('request', (r) => { if (r.method() === 'GET' && r.url().endsWith('/api/admin/talkgroups')) reads += 1; });
  await signIn(page, '#transcription');
  await page.route('**/api/admin/talkgroups/*/transcribe', (route) => route.fulfill(json({ error: 'The talkgroup is locked.' }, 409)));
  const fire = switchOf(page, /^Transcribe Metro Fire Dispatch/);
  await expect(page.getByText(/ on$/)).toHaveText('0 of 3 on');
  await fire.check();
  await expect(page.getByRole('alert')).toHaveText(/^Not changed: .+/);
  await expect(fire).not.toBeChecked();
  await expect(page.getByText(/ on$/)).toHaveText('0 of 3 on');
  await expect.poll(() => reads).toBe(2);
  await expect(page.getByRole('alert')).toHaveText(/^Not changed: .+/);
});

test('a count that comes back after a field changed is dropped', async ({ page }) => {
  await signIn(page, '#transcription');
  let release!: () => void;
  const held = new Promise<void>((resolve) => (release = resolve));
  await page.route('**/api/admin/jobs/queue*', async (route) => {
    await held;
    await route.fulfill(json({ count: 42, min_ms: 1000 }));
  });
  const counting = page.waitForRequest((r) => r.url().includes('/api/admin/jobs/queue'));
  const count = page.getByRole('button', { name: 'Count the calls' });
  await count.click();
  await counting;
  await page.getByLabel('From').fill('2031-07-01');
  release();
  // The button is free again when the answer has been handled.
  await expect(count).toBeEnabled();
  await expect(page.getByRole('button', { name: /^Queue / })).toHaveCount(0);
  await expect(page.getByText(/would be queued/)).toHaveCount(0);
});

test("the improvement module's part says why it is not shown while the module is off", async ({ page }) => {
  await signIn(page, '#transcription');
  await expect(page.getByText('Not shown: the improvement module is off (improvement_module.enabled).')).toBeVisible();
  await expect(page.getByRole('table', { name: "The improvement module's sets" })).toHaveCount(0);
});

test("with the module on, a set's report and runs show, and a candidate's run starts", async ({ page }) => {
  // The module is off in the seed, and a run would send calls to a server:
  // the test answers for the server.
  await page.route('**/api/admin/asr/sets', (route) =>
    route.fulfill(json([{ set: 'autumn', size: 50, labeled: 20, runs: 1 }, { set: 'spring', size: 10, labeled: 0, runs: 0 }])));
  await page.route('**/api/admin/asr/report?*', (route) =>
    route.fulfill(json({
      set: 'autumn', size: 50, labeled: 20,
      production: { wer: 0.152, errors: 38, words: 250, calls: 20, silent: 1, phantom: 0, summary: 'WER 15.2% (38 errors in 250 words, 20 calls)' },
      runs: [{ run_at: '2031-07-01T15:00:00Z', server: 'http://192.0.2.10:8000', model: 'large-v3', prompt: 'Unit numbers.', calls: 20, wer: 0.121, seconds_per_call: 1.25 }],
    })));
  const asked: unknown[] = [];
  await page.route('**/api/admin/jobs/asr-run', (route) => {
    const body = route.request().postDataJSON();
    asked.push(body);
    if (!body.model) return route.fulfill(json({ error: 'a model is needed' }, 400));
    return route.fulfill(json({ id: 9, count: 20 }, 201));
  });
  await signIn(page, '#transcription');
  const sets = page.getByRole('table', { name: "The improvement module's sets" });
  await expect(sets.locator('tbody tr')).toHaveCount(2);
  await sets.getByRole('button', { name: 'Show the report of autumn' }).click();
  await expect(page.getByText('Production on its calls: WER 15.2% (38 errors in 250 words, 20 calls)')).toBeVisible();
  const run = page.getByRole('table', { name: 'Runs on the set autumn' }).locator('tbody tr');
  await expect(run).toHaveCount(1);
  await expect(run).toContainText('large-v3 at http://192.0.2.10:8000');
  await expect(run).toContainText('Unit numbers.');
  await expect(run).toContainText('12.1%');
  await expect(run).toContainText('1.25');

  await page.getByLabel('Server').fill(' http://192.0.2.11:8000 ');
  await page.getByRole('button', { name: 'Start the run' }).click();
  await expect(page.getByRole('alert')).toHaveText('Not started: a model is needed');
  await page.getByLabel('Model').fill('medium');
  await page.getByRole('button', { name: 'Start the run' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Job #9 runs 20 calls of the set autumn; follow it in Jobs.' })).toBeVisible();
  expect(asked[1]).toEqual({ set: 'autumn', server: 'http://192.0.2.11:8000', model: 'medium' });
});

test('a failed read of the sets says so', async ({ page }) => {
  await page.route('**/api/admin/asr/sets', (route) => route.fulfill(json({ error: 'service unavailable', detail: 'the database does not answer' }, 503)));
  await signIn(page, '#transcription');
  await expect(page.getByRole('alert')).toHaveText('The sets could not be read: the database does not answer');
});

test('a failed read of a report says so', async ({ page }) => {
  await page.route('**/api/admin/asr/sets', (route) => route.fulfill(json([{ set: 'autumn', size: 1, labeled: 1, runs: 0 }])));
  await page.route('**/api/admin/asr/report?*', (route) => route.fulfill(json({ error: 'service unavailable', detail: 'the database does not answer' }, 503)));
  await signIn(page, '#transcription');
  await page.getByRole('button', { name: 'Show the report of autumn' }).click();
  await expect(page.getByRole('alert')).toHaveText('The report could not be read: the database does not answer');
});
