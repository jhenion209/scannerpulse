import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The admin page: signing in and out, the side menu and the Status area.
// Against e2e/seed.sql, where ingest has never run.

test.beforeEach(async ({ page }) => freshStorage(page));

const refuse = (status: number, detail: string) => ({
  status,
  contentType: 'application/json',
  body: JSON.stringify({ error: 'refused', detail }),
});
const statusHeading = (page: Page) => page.getByRole('heading', { name: 'Status', level: 1 });

test("the admin page asks to sign in, and shows a refusal in the server's words", async ({ page }) => {
  await page.route('**/api/auth/login', (route) => route.fulfill(refuse(401, 'the name or the password is wrong')));
  await page.goto('/admin');
  await page.getByLabel('Name').fill('someone');
  await page.getByLabel('Password').fill('not the password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('alert')).toHaveText('the name or the password is wrong');
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
  await expect(page.getByLabel('Password')).toHaveValue('');
});

test('an admin signs in, sees the status, and signs out', async ({ page }) => {
  await signIn(page);
  await expect(statusHeading(page)).toBeVisible();
  await expect(page.getByText('Ingest has never run against this database.')).toBeVisible();
  await expect(page.getByRole('table', { name: 'The newest calls' }).locator('tbody tr')).toHaveCount(20);
  await expect(page.getByRole('banner')).toContainText('e2e-admin');
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
  await page.reload();
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
});

test('single sign-on shows when it is set up, and no way in says how to turn one on', async ({ page }) => {
  let methods = { password: false, oidc: true };
  await page.route('**/api/auth/methods', (route) => route.fulfill({ json: methods }));
  await page.goto('/admin');
  await expect(page.getByRole('link', { name: 'Sign in with single sign-on' })).toHaveAttribute('href', '/api/auth/oidc');
  await expect(page.getByLabel('Password')).toHaveCount(0);
  methods = { password: false, oidc: false };
  await page.reload();
  await expect(page.getByRole('alert')).toContainText('scannerpulse admin password-signin on');
});

test('the status is read again every 15 seconds, and a failed read keeps the last one', async ({ page }) => {
  await page.clock.install();
  // What each read of the status said about itself: the timer's reads carry the poll header.
  const marks: (string | undefined)[] = [];
  let failing = false;
  await page.route('**/api/admin/status', async (route) => {
    marks.push(route.request().headers()['x-scannerpulse-poll']);
    if (failing) return route.fulfill(refuse(503, 'the database does not answer'));
    return route.continue();
  });
  await signIn(page);
  await expect(page.getByRole('heading', { name: 'Newest calls' })).toBeVisible();
  const before = marks.length;
  const readNow = page.getByRole('button', { name: 'Read now' });
  await readNow.focus();
  await page.clock.fastForward('00:15');
  await expect.poll(() => marks.length).toBe(before + 1);
  expect(marks[before], 'the timer reads as a poll, which the server does not count as use').toBe('1');
  await expect(readNow, 'the timer does not take the keyboard focus away').toBeFocused();
  failing = true;
  // A click while a read is still under way starts none, so it is made again until one is.
  await expect(async () => {
    await readNow.click();
    await expect(page.getByRole('alert')).toHaveText('The status could not be read: the database does not answer', { timeout: 1000 });
  }).toPass();
  expect(marks.at(-1), "the admin's own read is a plain one").toBeUndefined();
  await expect(page.getByRole('heading', { name: 'Newest calls' })).toBeVisible();
});

test('a session that ends brings the sign-in form back', async ({ page }) => {
  await signIn(page);
  await expect(statusHeading(page)).toBeVisible();
  // Read now starts no read while one is under way: let the first one end.
  await expect(page.getByText(/^Read at /)).toBeVisible();
  await page.route('**/api/admin/status', (route) => route.fulfill(refuse(401, 'sign in as an admin first')));
  await page.getByRole('button', { name: 'Read now' }).click();
  await expect(page.getByRole('alert').filter({ hasText: 'The session has ended' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
});

test('on a phone the areas open from the Menu button', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 760 });
  await signIn(page);
  const menu = page.getByRole('navigation', { name: 'Admin areas' });
  await expect(menu).toBeHidden();
  await page.getByRole('button', { name: 'Menu' }).click();
  await expect(menu).toBeVisible();
  // The bar wraps to two rows here: the open menu must not cover the second.
  const out = page.getByRole('button', { name: 'Sign out' });
  await expect(out).toBeVisible();
  const covered = await out.evaluate((button) => {
    const box = button.getBoundingClientRect();
    const top = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
    return !(top && button.contains(top));
  });
  expect(covered).toBe(false);
  await menu.getByRole('link', { name: 'Status' }).click();
  await expect(menu).toBeHidden();
  // Choosing the area already shown changes no address, yet closes the menu.
  await page.getByRole('button', { name: 'Menu' }).click();
  await expect(menu).toBeVisible();
  await menu.getByRole('link', { name: 'Status' }).click();
  await expect(menu).toBeHidden();
  const wide = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
  expect(wide).toBe(false);
});

test("a signed-in admin's edit on the main page carries the session's token", async ({ page }) => {
  await signIn(page);
  const me = await page.evaluate(() => fetch('/api/auth/me').then((r) => r.json()));
  let token: string | undefined;
  await page.route('**/api/calls/*/transcript', async (route) => {
    token = route.request().headers()['x-csrf-token'];
    const id = Number(route.request().url().match(/calls\/(\d+)/)![1]);
    await route.fulfill({ json: { id, transcript: 'Saved by an admin', origin: 'human' } });
  });
  await page.goto('/');
  const tr = page.locator('tbody tr').nth(2);
  await tr.locator('details.row-actions summary').click();
  await tr.getByRole('button', { name: 'Edit text' }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit the text' });
  await dialog.getByLabel('Text').fill('Saved by an admin');
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(dialog).toBeHidden();
  expect(token).toBe(me.csrf);
});

// A table's headings and short cells break only between words; only a cell that
// holds long text (a path, an address, a browser's name, an error) may break anywhere.
const SERVER_URL = 'http://speech-b.example.invalid:8000/v1/audio/transcriptions'; // 60 characters
const REFUSED_PATH = 'incoming/20261016/Metro_Police_One/20261016_140500_Metro_Police_One_TO_59_11.mp3'; // 80, no space
const AGENT_UNBROKEN = 'Mozilla/5.0-X11;Linux-x86_64-AppleWebKit/537.36-KHTML,like-Gecko-Chrome/130.0.0.0-Safari/537.36'.padEnd(200, 'z');
const AGENT_WORDS = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 '.repeat(2).slice(0, 200);
const longValues = {
  status: {
    maintenance: false,
    lease: {
      valid: true,
      holder: 'scannerpulse-a',
      expires_in: 12,
      heartbeats: [
        { host: 'scannerpulse-a', age: 3, expires_in: 12 },
        { host: 'scannerpulse-b', age: 5, expires_in: 10 },
      ],
    },
    ingest: {
      waiting: 0,
      rejected: 1,
      recent_rejected: [{ source: 'trial-in', rel_path: REFUSED_PATH, reason: 'the file is not an audio recording that can be read', seen_at: '2026-10-16T14:05:00Z' }],
    },
    queue: { queued: 0, due: 0, failed: 1, last_minute: 0 },
    failed: [{ call_id: 31, started_at: '2026-10-16T14:00:00Z', attempts: 3, last_error: 'the server answered 503 service unavailable twice' }],
    servers: [{ url: SERVER_URL, cooling_until: '2026-10-16T14:10:00Z', failed_rounds: 2, last_error: 'the connection was refused by the server', updated_at: '2026-10-16T14:00:00Z' }],
    totals: { calls_about: 35, transcripts_about: 20 },
    recent_calls: [{ id: 1, started_at: '2026-10-16T14:01:00Z', system: 'Metro', talkgroup: 59, talkgroup_name: 'Metro Police One', transcript_state: 'text' }],
    database: {
      primary: true,
      version: 'PostgreSQL 17.11',
      address: '192.0.2.53',
      port: 5432,
      standbys: [{ name: 'pgautofailover_standby_2', address: '198.51.100.250', state: 'streaming', sync_state: 'quorum', replay_lag_seconds: 0.2 }],
    },
    failover: { lease_held: true, jobs_running: 0, jobs_stuck: 0, calls_failed: 1, calls_filed_twice: 0, files_left_in_sources: 0 },
  },
  viewers: [
    { host: 'scannerpulse-a', address: '2001:db8:85a3::8a2e:370:7334', local: false, user_agent: AGENT_UNBROKEN, admin: 'e2e-admin', connected_at: '2026-10-16T13:51:20Z' },
    { host: 'scannerpulse-b', address: '2001:db8:85a3::8a2e:370:7334', local: false, user_agent: AGENT_WORDS, admin: null, connected_at: '2026-10-16T13:52:30Z' },
  ],
};

test("a table's headings and short values break between words, never inside one, at 360, 375 and 1280 px", async ({ page }) => {
  await page.route('**/api/admin/status', (route) => route.fulfill({ json: longValues.status }));
  await page.route('**/api/admin/viewers', (route) => route.fulfill({ json: longValues.viewers }));
  // A job that runs, so Status shows its table of what runs now.
  await page.route('**/api/admin/jobs', (route) =>
    route.fulfill({
      json: [{ id: 12, kind: 'purge_before', params: {}, state: 'running', done: 1200, total: 3400, message: '', started_by: 'root on scannerpulse-a',
        created_at: '2026-10-16T13:00:00Z', started_at: '2026-10-16T13:00:05Z', finished_at: null, updated_at: '2026-10-16T13:05:00Z', host: 'scannerpulse-a', stop_requested: false }],
    }),
  );
  await signIn(page);
  // Per table: its words that the browser breaks across lines, in a heading or in a cell without the `long` class
  // (a hyphen is a place to break between words), and the smallest text size of its cells.
  const broken = () =>
    page.evaluate(() => {
      const tables = [...document.querySelectorAll('.area table')];
      const found: string[] = [];
      let smallest = Infinity;
      for (const table of tables) {
        for (const cell of table.querySelectorAll('th, td')) smallest = Math.min(smallest, parseFloat(getComputedStyle(cell).fontSize));
        for (const cell of table.querySelectorAll('th, td:not(.long)')) {
          const texts = document.createTreeWalker(cell, NodeFilter.SHOW_TEXT);
          for (let node = texts.nextNode(); node; node = texts.nextNode()) {
            for (const word of (node.nodeValue ?? '').matchAll(/[^\s-]+/g)) {
              const range = document.createRange();
              range.setStart(node, word.index!);
              range.setEnd(node, word.index! + word[0].length);
              if (range.getClientRects().length > 1) found.push(`${table.querySelector('caption')?.textContent}: ${cell.tagName.toLowerCase()} "${word[0]}"`);
            }
          }
        }
      }
      return { tables: tables.length, found, smallest };
    });
  for (const width of [360, 375, 1280]) {
    await page.setViewportSize({ width, height: 800 });
    for (const [area, tables] of [['status', 7], ['viewers', 1]] as const) {
      await page.evaluate((h) => (location.hash = h), `#${area}`);
      await expect(page.getByRole('heading', { name: area === 'status' ? 'Newest calls' : 'Viewers' })).toBeVisible();
      if (area === 'viewers') await expect(page.getByRole('table', { name: 'The open pages' })).toBeVisible();
      const result = await broken();
      expect(result.tables, `${area} at ${width} px: the tables shown`).toBe(tables);
      expect(result.found, `${area} at ${width} px`).toEqual([]);
      expect(result.smallest, `${area} at ${width} px: the smallest text in a table, in px`).toBeGreaterThanOrEqual(14);
      const wide = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
      expect(wide, `${area} at ${width} px scrolls sideways`).toBe(false);
    }
  }
});

test('a long talkgroup name does not widen the Transcription or Data forms at 360 px', async ({ page }) => {
  // A select is as wide as its longest option: a grid track sized by it pushed the form past the page.
  const name = 'Northern Valley Regional Emergency Medical Services Dispatch Channel Two';
  expect(name.length).toBeGreaterThanOrEqual(70);
  await page.route('**/api/admin/talkgroups', (route) =>
    route.fulfill({ json: [{ id: 1, system: 'Metro', number: 101, name, transcribe: false, files_correct: true }] }));
  await page.setViewportSize({ width: 360, height: 800 });
  await signIn(page, '#transcription');
  for (const [area, heading, select] of [['transcription', 'Queue past calls', 'Talkgroup'], ['data', 'Merge', 'Talkgroup that goes']] as const) {
    await page.evaluate((h) => (location.hash = h), `#${area}`);
    await expect(page.getByRole('heading', { name: heading, level: 2 })).toBeVisible();
    // The list is read after the area opens: wait for its option (the first one is the prompt).
    await expect(page.getByLabel(select, { exact: true }).locator('option')).toHaveCount(2);
    // Not only the page: a form wider than the page but clipped must fail too.
    const past = await page.evaluate(() => {
      const found = document.documentElement.scrollWidth > window.innerWidth ? ['the page scrolls sideways'] : [];
      for (const e of document.querySelectorAll('.area form, .area select, .area label')) {
        const right = Math.round(e.getBoundingClientRect().right);
        if (right > window.innerWidth) found.push(`${e.tagName.toLowerCase()} ends at ${right}`);
      }
      return found;
    });
    expect(past, `${area} at 360 px`).toEqual([]);
  }
});

for (const [theme, width] of [['light', 1280], ['dark', 360]] as const) {
  test(`no accessibility fault signing in or in any area in the ${theme} theme at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.addInitScript((t) => localStorage.setItem('scannerpulse.theme', t), theme);
    const axe = async (where: string) => {
      const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
      expect(result.violations.map((v) => `${where} ${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
    };
    await page.goto('/admin');
    await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
    await axe('sign-in');
    await signIn(page);
    const links = page.getByRole('navigation', { name: 'Admin areas', includeHidden: true }).getByRole('link', { includeHidden: true });
    for (const href of await links.evaluateAll((as) => as.map((a) => a.getAttribute('href')!))) {
      await page.evaluate((h) => (location.hash = h), href);
      await expect(page.locator('h1')).toBeVisible();
      // Each area reads what it shows: wait until it has.
      await expect(page.getByText('Reading…')).toHaveCount(0);
      await axe(href);
    }
  });
}
