import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The Recorders area. The seed's recorder `live` is watched. A test adds a
// recorder in /tmp, which the server's machine has and its account may write
// in, wherever the browser runs; no ingest runs beside these tests, so
// nothing is taken from it. Each recorder added has a name of its own, since
// a removed recorder keeps its name.

test.beforeEach(async ({ page }) => freshStorage(page));

const card = (page: Page, name: string) =>
  page.getByRole('listitem').filter({ has: page.getByRole('heading', { name: new RegExp(`^${name} `) }) });

const unique = (stem: string) => `${stem}-${Date.now().toString(36)}`;

test('a recorder is added taken in when asked, brought in, and removed', async ({ page }) => {
  const name = unique('north');
  await signIn(page, '#recorders');
  await expect(page.getByRole('heading', { name: 'Recorders', level: 1 })).toBeVisible();
  await expect(card(page, 'live').getByText('Watched')).toBeVisible();
  await page.getByLabel('Name').fill(name);
  await page.getByLabel('Folder').fill('/tmp');
  await page.getByRole('radio', { name: 'only when asked' }).check();
  await page.getByRole('button', { name: 'Add the recorder' }).click();
  await expect(card(page, name).getByText('When asked')).toBeVisible();
  await card(page, name).getByRole('button', { name: `Bring in now ${name}` }).click();
  await expect(page.getByRole('status').filter({ hasText: `${name}'s files are brought in` })).toBeVisible();
  // No ingest runs beside these tests: the request waits to be listed.
  await expect(card(page, name).getByText('Bringing in')).toBeVisible();
  await card(page, name).getByRole('button', { name: `Remove… ${name}` }).click();
  await card(page, name).getByRole('button', { name: `Remove ${name}`, exact: true }).click();
  await expect(card(page, name)).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: `${name} is removed` })).toBeVisible();
});

test("a folder that cannot be used is refused in the server's words, and nothing is added", async ({ page }) => {
  await signIn(page, '#recorders');
  await page.getByLabel('Name').fill(unique('west'));
  await page.getByLabel('Folder').fill('/no/such/folder');
  await page.getByRole('button', { name: 'Add the recorder' }).click();
  await expect(page.getByRole('alert')).toHaveText(
    "Not added: /no/such/folder does not exist on this machine: mount the recorder's share there first",
  );
  await expect(page.getByRole('listitem')).toHaveCount(1);
});

test('a watched recorder is paused and watched again', async ({ page }) => {
  await signIn(page, '#recorders');
  await card(page, 'live').getByRole('button', { name: 'Pause live' }).click();
  await expect(card(page, 'live').getByText('Paused')).toBeVisible();
  await card(page, 'live').getByRole('button', { name: 'Watch live' }).click();
  await expect(card(page, 'live').getByText('Watched')).toBeVisible();
});

const json = (body: unknown, status = 200) => ({ status, contentType: 'application/json', body: JSON.stringify(body) });

/** Answers the POSTs under `path` with `answer`, keeping what each asked. */
async function answerPosts(page: Page, path: string, answer: (url: string) => { status: number; body?: unknown }) {
  const asked: { url: string; body: unknown }[] = [];
  await page.route(`**${path}`, (route) => {
    if (route.request().method() !== 'POST') return route.fallback();
    const url = new URL(route.request().url()).pathname;
    asked.push({ url, body: route.request().postDataJSON() });
    const { status, body } = answer(url);
    return route.fulfill(body === undefined ? { status } : json(body, status));
  });
  return asked;
}

const PREVIEW = {
  recorder: 'live',
  files: 1240,
  counts: { filed: 1, new_talkgroup: 0, named: 0, proposal: 1, staged: 1, rejected: 0, unreadable: 0 },
  sample: [
    { rel_path: '20310115_120000Metro__TO_101_FROM_4021.mp3', started_at: '2031-01-15T18:00:00Z', system: 'Metro', site: 'North',
      talkgroup: 101, talkgroup_name: 'Metro Fire Dispatch', radio_id: 4021, radio_alias: 'Medic 7', duration_ms: 3200, verdict: { kind: 'filed' } },
    { rel_path: '20310115_120100Metro__TO_102_FROM_4022.mp3', started_at: '2031-01-15T18:01:00Z', system: 'Metro', site: null,
      talkgroup: 102, talkgroup_name: 'Metro Police 1', radio_id: 4022, radio_alias: null, duration_ms: 1800,
      verdict: { kind: 'proposal', current: 'Metro Police One' } },
    { rel_path: '20310115_120200Metro-North__TO_7_FROM_4023.mp3', started_at: '2031-01-15T18:02:00Z', system: 'Metro-North', site: null,
      talkgroup: 7, talkgroup_name: 'Group 7', radio_id: null, radio_alias: null, duration_ms: 900, verdict: { kind: 'staged' } },
  ],
};

test('a preview shows what filing would do with each file of a sample', async ({ page }) => {
  await page.route('**/api/admin/recorders/*/preview', (route) => route.fulfill(json(PREVIEW)));
  await signIn(page, '#recorders');
  await card(page, 'live').getByRole('button', { name: 'Preview live' }).click();
  const preview = page.getByRole('region', { name: 'Preview of live' });
  await expect(preview.getByText('1,240 files in its folder, 3 read, spread over it: 2 filed (1 with a name to approve), 1 staged.')).toBeVisible();
  await expect(preview.getByRole('row')).toHaveCount(4);
  await expect(preview.getByRole('row').nth(2)).toContainText('Filed under Metro Police One; its name waits for your approval');
  await expect(preview.getByRole('row').nth(3)).toContainText('Staged: its system is unknown');
  await expect(preview.getByRole('row').nth(1)).toContainText('Metro / North');
  await expect(preview.getByRole('row').nth(1)).toContainText('Medic 7');
  await preview.getByRole('button', { name: 'Close the preview' }).click();
  await expect(preview).toHaveCount(0);
});

const WAITING = {
  waiting: 2,
  unknown: [
    { system: 'Metro-North', files: 412, recorders: ['live'], first_seen: '2031-01-14T20:02:00Z', last_seen: '2031-01-15T17:40:00Z',
      talkgroups: ['7 Group 7', '8 Group 8'] },
  ],
  proposals: [
    { talkgroup_id: 2, system: 'Metro', number: 102, current: 'Metro Police One', proposed: 'Metro Police 1', recorders: ['live'],
      calls: 230, first_seen: '2031-01-15T15:15:00Z', last_seen: '2031-01-15T17:38:00Z' },
  ],
  systems: ['Metro', 'Valley'],
};

test('what waits for you is counted in the menu and decided in the Recorders area', async ({ page }) => {
  await page.route('**/api/admin/names', (route) => route.fulfill(json(WAITING)));
  const asked = await answerPosts(page, '/api/admin/{names,staging}/*', (url) =>
    url.endsWith('/discard') ? { status: 200, body: { discarded: 412 } } : { status: 204 },
  );
  await signIn(page, '#status');
  await expect(page.getByRole('navigation', { name: 'Admin areas' }).getByRole('link', { name: 'Recorders, 2 waiting for you' })).toBeVisible();
  await page.getByRole('link', { name: 'Recorders, 2 waiting for you' }).click();
  const unknown = page.getByRole('listitem').filter({ has: page.getByRole('heading', { name: 'Metro-North' }) });
  await expect(unknown).toContainText('412');
  await expect(unknown).toContainText('7 Group 7, 8 Group 8');
  await unknown.getByRole('button', { name: 'New system Metro-North' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Metro-North is a new system' })).toBeVisible();
  await unknown.getByLabel('Same as, for Metro-North').selectOption('Metro');
  await unknown.getByRole('button', { name: 'Map Metro-North' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Metro-North is Metro from now on' })).toBeVisible();
  await unknown.getByRole('button', { name: 'Discard… Metro-North' }).click();
  await expect(unknown.getByRole('button', { name: 'Discard its files' })).toBeDisabled();
  await unknown.getByLabel('Type Metro-North to confirm').fill('Metro-North');
  await unknown.getByRole('button', { name: 'Discard its files' }).click();
  await expect(page.getByRole('status').filter({ hasText: '412 staged files of Metro-North are deleted.' })).toBeVisible();
  const proposal = page.getByRole('listitem').filter({ has: page.getByRole('heading', { name: 'Metro 102: Metro Police One → Metro Police 1' }) });
  await proposal.getByRole('button', { name: 'Approve Metro Police 1' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Metro 102 is Metro Police 1' })).toBeVisible();
  await proposal.getByRole('button', { name: 'Dismiss Metro Police 1' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Metro Police 1 is dismissed for Metro 102' })).toBeVisible();
  expect(asked).toEqual([
    { url: '/api/admin/staging/new-system', body: { system: 'Metro-North' } },
    { url: '/api/admin/staging/map', body: { system: 'Metro-North', to: 'Metro' } },
    { url: '/api/admin/staging/discard', body: { system: 'Metro-North', confirm: 'Metro-North' } },
    { url: '/api/admin/names/approve', body: { talkgroup_id: 2, name: 'Metro Police 1' } },
    { url: '/api/admin/names/dismiss', body: { talkgroup_id: 2, name: 'Metro Police 1' } },
  ]);
});

test("a refused decision says why in the server's words", async ({ page }) => {
  await page.route('**/api/admin/names', (route) => route.fulfill(json(WAITING)));
  await answerPosts(page, '/api/admin/names/*', () => ({
    status: 400,
    body: { error: 'bad request', detail: 'talkgroup Metro/101 has that name already: merge into it instead' },
  }));
  await signIn(page, '#recorders');
  const proposal = page.getByRole('listitem').filter({ has: page.getByRole('heading', { name: /^Metro 102/ }) });
  await proposal.getByRole('button', { name: 'Approve Metro Police 1' }).click();
  await expect(proposal.getByRole('alert')).toHaveText(
    'Not decided: talkgroup Metro/101 has that name already: merge into it instead',
  );
});

test('the preview and what waits have no accessibility fault and no sideways scroll at 360 px', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.addInitScript(() => localStorage.setItem('scannerpulse.theme', 'dark'));
  const long = `${'a'.repeat(80)}.mp3`;
  await page.route('**/api/admin/recorders/*/preview', (route) =>
    route.fulfill(json({ ...PREVIEW, sample: PREVIEW.sample.map((f) => ({ ...f, rel_path: `${f.rel_path}/${long}` })) })));
  const unknown = { ...WAITING.unknown[0], system: `Metro-North-${'b'.repeat(40)}` };
  await page.route('**/api/admin/names', (route) => route.fulfill(json({ ...WAITING, unknown: [unknown] })));
  await signIn(page, '#recorders');
  await card(page, 'live').getByRole('button', { name: 'Preview live' }).click();
  await expect(page.getByRole('region', { name: 'Preview of live' })).toBeVisible();
  await page.getByRole('button', { name: `Discard… ${unknown.system}` }).click();
  await expect(page.getByLabel(`Type ${unknown.system} to confirm`)).toBeVisible();
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
  expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)).toBe(false);
});
