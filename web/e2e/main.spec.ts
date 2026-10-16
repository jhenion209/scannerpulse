import AxeBuilder from '@axe-core/playwright';
import { readFileSync } from 'node:fs';
import { expect, test, type Locator, type Page } from '@playwright/test';

// Against e2e/seed.sql: calls 1 to 30 in the last 90 minutes (call 1 the
// newest), 31 to 35 three days ago; call 7 has no audio file; call 5's text
// holds markup.

const rows = (page: Page) => page.locator('tbody tr');
const ids = (page: Page) => rows(page).evaluateAll((trs) => trs.map((tr) => Number((tr as HTMLElement).dataset.id)));
const playingRow = (page: Page) => page.locator('tr.playing');

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('e2e')) {
      localStorage.clear();
      sessionStorage.setItem('e2e', '1');
    }
  });
});

test('the first view is the last 24 hours, newest first', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await expect(page.getByText('30 calls')).toBeVisible();
  await page.getByLabel('All dates').check();
  await expect(rows(page)).toHaveCount(35);
  await expect(page).toHaveURL(/all=1/);
});

test('pages are read on the server, by size, by next and by number', async ({ page }) => {
  await page.goto('/?all=1');
  await page.getByLabel('Rows').selectOption('10');
  await expect(rows(page)).toHaveCount(10);
  await expect(page.getByText('of 4')).toBeVisible();
  const first = await ids(page);
  await page.getByRole('button', { name: 'Next' }).click();
  await expect.poll(() => ids(page)).not.toEqual(first);
  await page.getByLabel('Page number').fill('4');
  await page.getByLabel('Page number').press('Enter');
  await expect(rows(page)).toHaveCount(5);
  await expect(page.getByRole('button', { name: 'Next' })).toBeDisabled();
  await page.getByRole('button', { name: 'First' }).click();
  await expect.poll(() => ids(page)).toEqual(first);
  // The size is the listener's: it is still 10 after a reload.
  await page.reload();
  await expect(rows(page)).toHaveCount(10);
});

test('a talkgroup filter is in the URL, and the link reproduces it', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('Find a talkgroup').fill('ambul');
  await page.getByRole('checkbox', { name: 'Valley Ambulance' }).check();
  await expect(rows(page)).toHaveCount(10);
  await expect(page.locator('td.tg')).toHaveCount(10);
  await expect(page.locator('td.tg').filter({ hasNotText: 'Valley Ambulance' })).toHaveCount(0);
  const url = page.url();
  expect(url).toMatch(/talkgroups=\d+/);
  await page.goto(url);
  await expect(rows(page)).toHaveCount(10);
  await expect(page.getByRole('checkbox', { name: 'Valley Ambulance' })).toBeChecked();
});

test('a favorite asks for its talkgroups', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('Favorite').selectOption({ label: 'Fire and ambulance' });
  await expect(rows(page)).toHaveCount(20);
  await expect(page).toHaveURL(/favorite=\d+/);
});

test('text is shown as text, never as markup, and opens in full', async ({ page }) => {
  await page.goto('/');
  const row = page.locator('tr', { has: page.getByText('<b>not bold</b>') });
  await expect(row).toHaveCount(1);
  await expect(row.locator('b')).toHaveCount(0);
  const long = rows(page).filter({ hasText: 'Engine 9 ' });
  await long.getByRole('button', { name: 'More' }).click();
  await expect(long.getByRole('button', { name: 'Less' })).toHaveAttribute('aria-expanded', 'true');
});

test('the player steps with N and P, stops at the edges and passes over a call that fails', async ({ page }) => {
  await page.goto('/');
  // The calls are 2 s long: without continuous play none moves on by itself.
  await page.getByLabel('Continuous').uncheck();
  // The rows are read only once the list is there.
  await expect(rows(page)).toHaveCount(30);
  const all = await ids(page);
  await rows(page).nth(2).getByRole('button', { name: /^Play/ }).click();
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[2]));
  await page.keyboard.press('n');
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[1]));
  await page.keyboard.press('n');
  await page.keyboard.press('n');
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[0]));
  await expect(page.getByRole('button', { name: 'Newer call (N)' })).toBeDisabled();
  // Rows 6, 7, 8 are calls 6, 7 and 8 (newest first); call 7 has no file.
  await rows(page).nth(5).getByRole('button', { name: /^Play/ }).click();
  await page.keyboard.press('p');
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[7]));
  await expect(rows(page).nth(6)).toHaveClass(/played/);
  // Keys typed into a field are the field's.
  const field = page.getByLabel('Find a talkgroup');
  await field.focus();
  // One key at a time: N then P would step away and back to the same row.
  await field.pressSequentially('n');
  await expect(field).toHaveValue('n');
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[7]));
  await field.pressSequentially('p');
  await expect(field).toHaveValue('np');
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[7]));
});

test('continuous play moves to the newer call when one ends', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const all = await ids(page);
  await rows(page).nth(4).getByRole('button', { name: /^Play/ }).click();
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[3]), { timeout: 10_000 });
});

test('the theme is chosen before the page draws, and kept', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: /^Theme/ }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.reload({ waitUntil: 'commit' });
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
});

for (const theme of ['light', 'dark']) {
  for (const width of [1280, 360]) {
    test(`no accessibility fault in the ${theme} theme at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 800 });
      await page.addInitScript((t) => localStorage.setItem('scannerpulse.theme', t), theme);
      await page.goto('/');
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      await expect(rows(page)).toHaveCount(30);
      const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
      expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    });
  }
}

test('on a phone the filters open from the Filters button', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto('/');
  await expect(page.getByLabel('Find a talkgroup')).toBeHidden();
  await page.getByRole('button', { name: 'Filters' }).click();
  await expect(page.getByLabel('Find a talkgroup')).toBeVisible();
});

test('the page number is in the URL, and the link opens that page', async ({ page }) => {
  await page.goto('/?all=1');
  await page.getByLabel('Rows').selectOption('10');
  await expect(rows(page)).toHaveCount(10);
  const first = await ids(page);
  await page.getByRole('button', { name: 'Next' }).click();
  await expect.poll(() => ids(page)).not.toEqual(first);
  await expect(page).toHaveURL(/page=2/);
  const second = await ids(page);
  await page.goto(page.url());
  await expect(page.getByLabel('Page number')).toHaveValue('2');
  await expect.poll(() => ids(page)).toEqual(second);
  await expect(page).toHaveURL(/page=2/);
  // The first page leaves no page in the URL.
  await page.getByRole('button', { name: 'First' }).click();
  await expect.poll(() => ids(page)).toEqual(first);
  await expect(page).not.toHaveURL(/page=/);
});

test('a link with a favorite and a page lands on that page', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('scannerpulse.pageSize', '10'));
  await page.goto('/');
  const id = await page.getByLabel('Favorite').locator('option', { hasText: 'Fire and ambulance' }).getAttribute('value');
  await page.goto(`/?favorite=${id}&page=2`);
  await expect(page.getByLabel('Page number')).toHaveValue('2');
  await expect(rows(page)).toHaveCount(10);
  await expect(page.getByText('20 calls')).toBeVisible();
  await expect(page).toHaveURL(/page=2/);
});

test('a link with a favorite and a page waits for slow favorites', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('scannerpulse.pageSize', '10'));
  await page.goto('/');
  const id = await page.getByLabel('Favorite').locator('option', { hasText: 'Fire and ambulance' }).getAttribute('value');
  await page.route('**/api/favorites', async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 1500));
    await route.continue();
  });
  await page.goto(`/?favorite=${id}&page=2`);
  await expect(page.getByLabel('Page number')).toHaveValue('2', { timeout: 10_000 });
  await expect(rows(page)).toHaveCount(10);
  await expect(page.getByText('20 calls')).toBeVisible();
  await expect(page).toHaveURL(/page=2/);
});

const columnMenu = (page: Page) => page.locator('details.columns');
const openColumns = async (page: Page) => {
  await columnMenu(page).locator('summary').click();
  await expect(columnMenu(page).getByRole('group', { name: 'Columns' })).toBeVisible();
};
const columnBox = (page: Page, name: string) => columnMenu(page).getByRole('checkbox', { name, exact: true });

test('the Columns menu hides and shows columns, keeps the choice, and resets it', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  // Today's six columns on a desk; none of the four extras.
  for (const name of ['Time', 'Talkgroup', 'Site', 'Source', 'Length', 'Text']) {
    await expect(page.getByRole('columnheader', { name, exact: true })).toBeVisible();
  }
  for (const name of ['System', 'Call ID', 'Text source', 'Talkgroup number']) {
    await expect(page.getByRole('columnheader', { name, exact: true })).toHaveCount(0);
  }
  await openColumns(page);
  await expect(columnBox(page, 'Site')).toBeChecked();
  await expect(columnBox(page, 'Call ID')).not.toBeChecked();
  await expect(columnMenu(page).getByRole('button', { name: 'Reset to default' })).toBeDisabled();

  await columnBox(page, 'Site').uncheck();
  await columnBox(page, 'Call ID').check();
  await expect(page.getByRole('columnheader', { name: 'Site', exact: true })).toHaveCount(0);
  await expect(page.locator('td.c-site')).toHaveCount(0);
  await expect(page.getByRole('columnheader', { name: 'Call ID', exact: true })).toBeVisible();
  const shownIds = await page.locator('td.c-id').allInnerTexts();
  expect(shownIds).toEqual((await ids(page)).map(String));

  // The choice is the listener's: it is still there after a reload.
  await page.reload();
  await expect(rows(page)).toHaveCount(30);
  await expect(page.getByRole('columnheader', { name: 'Site', exact: true })).toHaveCount(0);
  await expect(page.getByRole('columnheader', { name: 'Call ID', exact: true })).toBeVisible();

  await openColumns(page);
  await columnMenu(page).getByRole('button', { name: 'Reset to default' }).click();
  await expect(page.getByRole('columnheader', { name: 'Site', exact: true })).toBeVisible();
  await expect(page.getByRole('columnheader', { name: 'Call ID', exact: true })).toHaveCount(0);
  await expect(columnMenu(page).getByRole('button', { name: 'Reset to default' })).toBeDisabled();
});

test('the Columns menu closes with Escape and gives the focus back to its button', async ({ page }) => {
  await page.goto('/');
  await openColumns(page);
  await columnBox(page, 'Site').focus();
  await page.keyboard.press('Escape');
  await expect(columnMenu(page).getByRole('group', { name: 'Columns' })).toBeHidden();
  await expect(columnMenu(page).locator('summary')).toBeFocused();
});

test('Tab moves the focus on and closes the Columns menu, so it cannot cover the focused control', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto('/');
  await openColumns(page);
  // Nothing is saved yet, so "Reset to default" is disabled: the last checkbox is the last stop in the menu.
  await columnBox(page, 'Text').focus();
  await page.keyboard.press('Tab');
  await expect(columnMenu(page).getByRole('group', { name: 'Columns' })).toBeHidden();
  await expect(page.getByLabel('Rows')).toBeFocused();
});

test('on a desk the list still fits when the Text column is hidden', async ({ page }) => {
  await page.goto('/');
  await openColumns(page);
  await columnBox(page, 'Text').uncheck();
  await expect(page.getByRole('columnheader', { name: 'Text', exact: true })).toHaveCount(0);
  await expect(rows(page).first().getByRole('button', { name: /^Play/ })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(1280);
});

test('on a phone the chosen extra columns are labelled values under the text', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  // Nothing extra by default: as before, two lines and no labels.
  await expect(page.locator('td.chips')).toHaveCount(0);
  await expect(page.locator('tr[data-id] td.extra:visible')).toHaveCount(0);

  await openColumns(page);
  await columnBox(page, 'Source').check();
  await columnBox(page, 'Call ID').check();
  const first = rows(page).first();
  // The plain cells stay out of a phone's layout (and out of what a screen reader reads).
  await expect(first.locator('td.c-source')).toBeHidden();
  const id = await first.getAttribute('data-id');
  const chips = first.locator('td.chips');
  await expect(chips).toContainText(`Call ID ${id}`);
  await expect(chips).toContainText(/Source \d+/);
  // The seed leaves the site empty: an empty value shows no label.
  await columnBox(page, 'Site').check();
  await expect(columnBox(page, 'Site')).toBeChecked();
  await expect(chips).toContainText(`Call ID ${id}`);
  await expect(chips).not.toContainText('Site');
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(360);

  // Hiding a core column on a phone leaves the others on their line.
  await columnBox(page, 'Length').uncheck();
  await expect(first.locator('td.c-length')).toHaveCount(0);
  await expect(first.locator('td.c-talkgroup')).toBeVisible();
});

for (const [theme, width] of [['light', 1280], ['dark', 360]] as const) {
  test(`no accessibility fault with the Columns menu open in the ${theme} theme at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.addInitScript((t) => localStorage.setItem('scannerpulse.theme', t), theme);
    await page.goto('/');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expect(rows(page)).toHaveCount(30);
    await openColumns(page);
    const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
    expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
  });
}

test('the top bar shows the status: the calls, the newest call\'s age and the server\'s time', async ({ page }) => {
  await page.goto('/');
  const chips = page.getByRole('list', { name: 'Status' });
  await expect(chips).toContainText('35 calls');
  await expect(chips).toContainText(/Newest call \d+ (s|min|h|d)/);
  await expect(chips.locator('time')).toHaveText(/^\d\d:\d\d:\d\d \S+$/);
  // The seed turns neither maintenance nor transcription on.
  await expect(chips).not.toContainText('Maintenance');
  await expect(chips).not.toContainText('Queue');
});

const actionsOf = async (page: Page, row: number) => {
  await expect(rows(page)).toHaveCount(30);
  const tr = rows(page).nth(row);
  await tr.locator('details.row-actions summary').click();
  return tr;
};

test('a call\'s text is edited in a dialog, checked as the server checks it, and kept', async ({ page }) => {
  await page.goto('/');
  // Row 3 is call 3, which has a text.
  const tr = await actionsOf(page, 2);
  await tr.getByRole('button', { name: 'Edit text' }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit the text' });
  await expect(dialog).toBeVisible();
  const box = dialog.getByLabel('Text');
  await box.fill('a <b> tag');
  await expect(dialog.getByRole('alert')).toHaveText('a transcript cannot contain < or >');
  await expect(dialog.getByRole('button', { name: 'Save' })).toBeDisabled();
  const text = `Corrected by a test ${Date.now()}`;
  await box.fill(text);
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(dialog).toBeHidden();
  await expect(tr.locator('td.c-text')).toContainText(text);
  await page.reload();
  await expect(rows(page).nth(2).locator('td.c-text')).toContainText(text);
});

test('a refused save shows the server\'s own words and keeps the dialog open', async ({ page }) => {
  await page.route('**/api/calls/*/transcript', (route) =>
    route.fulfill({
      status: 503,
      contentType: 'application/json',
      body: JSON.stringify({ error: 'service unavailable', detail: 'the site is under maintenance: saving is paused' }),
    }),
  );
  await page.goto('/');
  const tr = await actionsOf(page, 2);
  await tr.getByRole('button', { name: 'Edit text' }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit the text' });
  await dialog.getByLabel('Text').fill('A text that will not be saved');
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(dialog.getByRole('alert')).toHaveText('The text was not saved: the site is under maintenance: saving is paused');
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('a call downloads under its own file name', async ({ page }) => {
  await page.goto('/');
  const tr = await actionsOf(page, 0);
  const id = await tr.getAttribute('data-id');
  const [file] = await Promise.all([
    page.waitForEvent('download'),
    tr.getByRole('button', { name: 'Download' }).click(),
  ]);
  // The seed names call n's file call-n.mp3; row 1 is call 1.
  expect(file.suggestedFilename()).toBe('call-1.mp3');
  expect(file.url()).toMatch(new RegExp(`/audio/${id}$`));
});

test('sharing a call copies a link to its own page', async ({ page, context, baseURL }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  const tr = await actionsOf(page, 0);
  const id = await tr.getAttribute('data-id');
  await tr.getByRole('button', { name: 'Share link' }).click();
  const url = `${baseURL}/calls/${id}`;
  await expect(page.getByRole('status')).toHaveText(`Link copied: ${url}`);
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(url);
});

test('where the browser will not copy, the link is shown selected to copy by hand', async ({ page, baseURL }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: () => Promise.reject(new Error('not allowed')) },
    });
  });
  await page.goto('/');
  const tr = await actionsOf(page, 0);
  const id = await tr.getAttribute('data-id');
  await tr.getByRole('button', { name: 'Share link' }).click();
  const dialog = page.getByRole('dialog', { name: 'Link to this call' });
  const field = dialog.getByLabel('Copy the link');
  await expect(field).toHaveValue(`${baseURL}/calls/${id}`);
  expect(await field.evaluate((input: HTMLInputElement) => input.selectionEnd - input.selectionStart)).toBe(
    `${baseURL}/calls/${id}`.length,
  );
});

test('a shared link opens the call above the list, ready to play', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const id = (await ids(page))[4];
  await page.goto(`/calls/${id}`);
  const card = page.getByRole('region', { name: 'Shared call' });
  await expect(card).toBeVisible();
  await expect(card).toContainText(/Metro|Valley/);
  await card.getByRole('button', { name: /^Play/ }).click();
  await expect(page.locator('footer.player .now')).not.toHaveText('Nothing playing');
  await card.getByRole('button', { name: 'Close' }).click();
  await expect(card).toHaveCount(0);
  await expect(page).toHaveURL(/\/(\?.*)?$/);
});

test('a link to a call that is gone says so', async ({ page }) => {
  await page.goto('/calls/987654321');
  await expect(page.getByRole('region', { name: 'Shared call' })).toContainText('That call is not in the archive.');
});

test('no accessibility fault with the edit dialog open', async ({ page }) => {
  await page.goto('/');
  const tr = await actionsOf(page, 2);
  await tr.getByRole('button', { name: 'Edit text' }).click();
  await expect(page.getByRole('dialog', { name: 'Edit the text' })).toBeVisible();
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
  expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
});

test('selection mode ticks calls of this page only, and holds new calls back while on', async ({ page }) => {
  await page.goto('/?all=1');
  await page.getByLabel('Rows').selectOption('10');
  await expect(rows(page)).toHaveCount(10);
  // Unpaused (All dates counts as paused, so this view is read without it first).
  await page.getByLabel('All dates').uncheck();
  await expect(rows(page)).toHaveCount(10);
  const pause = page.getByLabel('Pause updates');
  await expect(pause).not.toBeChecked();
  const toggle = page.getByRole('button', { name: 'Select calls' });
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-pressed', 'true');
  await expect(pause).toBeChecked();
  await expect(page.locator('nav.pager .chosen')).toHaveText('0 selected');
  const boxes = page.locator('tbody td.c-select input');
  await expect(boxes).toHaveCount(10);
  await boxes.nth(0).check();
  await boxes.nth(2).check();
  await expect(page.locator('nav.pager .chosen')).toHaveText('2 selected');
  const all = page.getByLabel('Select every call on this page');
  expect(await all.evaluate((box: HTMLInputElement) => box.indeterminate)).toBe(true);
  await all.check();
  await expect(page.locator('nav.pager .chosen')).toHaveText('10 selected');
  await all.uncheck();
  await expect(page.locator('nav.pager .chosen')).toHaveText('0 selected');
  // Another page starts with nothing chosen.
  await boxes.nth(1).check();
  await expect(page.locator('nav.pager .chosen')).toHaveText('1 selected');
  const first = await ids(page);
  await page.getByRole('button', { name: 'Next' }).click();
  await expect.poll(() => ids(page)).not.toEqual(first);
  await expect(page.locator('nav.pager .chosen')).toHaveText('0 selected');
  // Leaving selection puts Pause back as it was.
  await toggle.click();
  await expect(boxes).toHaveCount(0);
  await expect(pause).not.toBeChecked();
});

test('on a phone, selection mode fits the screen', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await page.getByRole('button', { name: 'Select calls' }).click();
  await expect(page.locator('tbody td.c-select input')).toHaveCount(30);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(360);
});

const exportMenu = (page: Page) => page.locator('details.export');
const exportFile = async (page: Page, item: RegExp) => {
  await exportMenu(page).locator('summary').click();
  const [file] = await Promise.all([page.waitForEvent('download'), exportMenu(page).getByRole('button', { name: item }).click()]);
  return file;
};
const NAME = /^\d{8}_\d{6}-\d{8}_\d{6}_ScannerPulse_TG-[^/]+\.(xlsx|csv|mp3)$/;

test('Excel covers every match, built on the server, under the export name', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const file = await exportFile(page, /^Excel/);
  expect(file.suggestedFilename()).toMatch(NAME);
  expect(file.suggestedFilename().endsWith('.xlsx')).toBe(true);
  // An .xlsx is a zip.
  expect(readFileSync(await file.path()).subarray(0, 2).toString()).toBe('PK');
});

test('CSV covers the page, or the chosen calls, with the text as it is', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const file = await exportFile(page, /^CSV — this page/);
  expect(file.suggestedFilename()).toMatch(NAME);
  const lines = readFileSync(await file.path(), 'utf8').split('\r\n').filter(Boolean);
  expect(lines[0]).toBe('\uFEFFDate,Time,Talkgroup,Site,Source,Length (s),Text');
  expect(lines).toHaveLength(31);
  expect(lines.some((l) => l.endsWith(',<b>not bold</b> & kept as typed'))).toBe(true);

  await page.getByRole('button', { name: 'Select calls' }).click();
  const boxes = page.locator('tbody td.c-select input');
  await boxes.nth(0).check();
  await boxes.nth(1).check();
  const chosen = await exportFile(page, /^CSV — 2 selected/);
  expect(readFileSync(await chosen.path(), 'utf8').split('\r\n').filter(Boolean)).toHaveLength(3);
});

test('call audio joins every match, and says which calls could not be read', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const file = await exportFile(page, /^Call audio — every match/);
  expect(file.suggestedFilename()).toMatch(NAME);
  expect(readFileSync(await file.path()).length).toBeGreaterThan(1000);
  // The seed's call 7 has no file.
  await expect(page.getByRole('status')).toHaveText('1 call could not be read and was left out.');
});

test('call audio of the chosen calls joins only those', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await page.getByRole('button', { name: 'Select calls' }).click();
  const boxes = page.locator('tbody td.c-select input');
  await boxes.nth(0).check();
  await boxes.nth(1).check();
  const file = await exportFile(page, /^Call audio — 2 selected/);
  expect(file.suggestedFilename()).toMatch(NAME);
  const joined = readFileSync(await file.path()).length;
  // Two silent 2-second calls of 56 frames of 144 bytes each.
  expect(joined).toBe(2 * 56 * 144);
});

test('printing keeps the calls only, names the PDF by the export name, and prints only the chosen calls', async ({ page }) => {
  await page.addInitScript(() => {
    (window as unknown as { printed: string[] }).printed = [];
    window.print = () => (window as unknown as { printed: string[] }).printed.push(document.title);
  });
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const title = await page.title();
  await page.getByRole('button', { name: 'Select calls' }).click();
  await page.locator('tbody td.c-select input').nth(3).check();
  await exportMenu(page).locator('summary').click();
  await exportMenu(page).getByRole('button', { name: /^PDF — 1 selected/ }).click();
  const printed = await page.evaluate(() => (window as unknown as { printed: string[] }).printed);
  expect(printed).toHaveLength(1);
  expect(`${printed[0]}.csv`).toMatch(NAME);
  // As printed: the controls are gone and only the chosen call is left.
  await page.emulateMedia({ media: 'print' });
  await expect(page.locator('header.bar')).toBeHidden();
  await expect(page.locator('nav.pager')).toBeHidden();
  await expect(page.locator('tbody tr:visible')).toHaveCount(1);
  await page.emulateMedia({ media: 'screen' });
  await page.evaluate(() => window.dispatchEvent(new Event('afterprint')));
  await expect(page).toHaveTitle(title);
});

const panel = (page: Page) => page.getByRole('dialog', { name: 'Search the archive' });
const panelRows = (page: Page) => panel(page).locator('tbody tr');
const exactCount = async (page: Page, words: string) =>
  (await (await page.request.get(`/api/calls?q=${encodeURIComponent(words)}&exact=true&size=10`)).json()).total as number;

test('the bar\'s search opens the panel, with every date searched and an exact count', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const expected = await exactCount(page, 'responding');
  expect(expected).toBeGreaterThan(10);
  await page.getByLabel('Words to search for').fill('responding');
  await page.getByRole('button', { name: 'Search', exact: true }).click();
  await expect(panel(page)).toBeVisible();
  await expect(panel(page)).toContainText(`${expected} matches`);
  await expect(page).toHaveURL(/search=responding/);
  // The live list behind it is the live list still.
  await expect(rows(page)).toHaveCount(30 + (await panelRows(page).count()));
});

test('a link with words opens the panel and searches them', async ({ page }) => {
  const expected = await exactCount(page, 'station');
  await page.goto('/?search=station');
  await expect(panel(page)).toBeVisible();
  await expect(panel(page)).toContainText(`${expected} matches`);
  await expect(panel(page).getByLabel('Words')).toHaveValue('station');
});

test('a result holds the live player, which goes on when the result ends', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await page.getByLabel('Continuous').uncheck();
  const live = page.locator('footer.player audio');
  const result = page.locator('dialog.panel audio');
  const paused = (audio: typeof live) => audio.evaluate((a: HTMLAudioElement) => a.paused);
  await rows(page).nth(0).getByRole('button', { name: /^Play/ }).click();
  await expect.poll(() => paused(live)).toBe(false);
  await page.getByLabel('Words to search for').fill('responding');
  await page.getByRole('button', { name: 'Search', exact: true }).click();
  await expect(panelRows(page).first()).toBeVisible();
  await panelRows(page).first().getByRole('button', { name: /^Play/ }).click();
  await expect.poll(() => paused(result)).toBe(false);
  await expect.poll(() => paused(live)).toBe(true);
  // The result is 2 s long: when it ends, the live call goes on.
  await expect.poll(() => paused(live), { timeout: 8000, intervals: [100] }).toBe(false);
});

test('starting the live player by hand stops the result', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await page.getByLabel('Words to search for').fill('responding');
  await page.getByRole('button', { name: 'Search', exact: true }).click();
  await panelRows(page).first().getByRole('button', { name: /^Play/ }).click();
  const result = page.locator('dialog.panel audio');
  await expect.poll(() => result.evaluate((a: HTMLAudioElement) => a.paused)).toBe(false);
  await page.locator('footer.player audio').evaluate((a: HTMLAudioElement) => {
    a.src = a.src || '/audio/1';
    return a.play();
  }).catch(() => {});
  await expect.poll(() => result.evaluate((a: HTMLAudioElement) => a.paused)).toBe(true);
  await expect(panel(page)).toContainText('No result playing');
});

test('Escape closes the panel, takes the words out of the address and gives the focus back', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('Words to search for').fill('station');
  const button = page.getByRole('button', { name: 'Search', exact: true });
  await button.click();
  await expect(panel(page)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(panel(page)).toBeHidden();
  await expect(page).not.toHaveURL(/search=/);
  await expect(button).toBeFocused();
});

test('the panel\'s results export to Excel', async ({ page }) => {
  await page.goto('/?search=responding');
  await expect(panelRows(page).first()).toBeVisible();
  const [file] = await Promise.all([
    page.waitForEvent('download'),
    panel(page).getByRole('button', { name: 'Excel — every match' }).click(),
  ]);
  expect(file.suggestedFilename()).toMatch(/ScannerPulse.*\.xlsx$/);
});

for (const width of [1280, 360]) {
  test(`no accessibility fault with the search panel open at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.goto('/?search=responding');
    await expect(panelRows(page).first()).toBeVisible();
    const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
    expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(' ')}`)).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  });
}

test('the panel\'s words can be changed after a search, and typing alone searches nothing', async ({ page }) => {
  const expected = await exactCount(page, 'responding');
  await page.goto('/?search=station');
  const words = panel(page).getByLabel('Words');
  await expect(panelRows(page).first()).toBeVisible();
  const asked: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname === '/api/calls' && url.searchParams.has('q')) asked.push(url.searchParams.get('q') ?? '');
  });
  await words.fill('');
  await words.pressSequentially('responding');
  await expect(words).toHaveValue('responding');
  expect(asked).toEqual([]);
  await panel(page).getByRole('button', { name: 'Search', exact: true }).click();
  await expect(panel(page)).toContainText(`${expected} matches`);
  await expect(words).toHaveValue('responding');
  await expect(page).toHaveURL(/search=responding/);
  expect(asked).toEqual(['responding']);
});

test('a page turn and the Excel file use the search on screen, not fields edited since', async ({ page }) => {
  await page.goto('/?search=responding');
  await expect(panelRows(page).first()).toBeVisible();
  await panel(page).getByLabel('Rows').selectOption('10');
  await expect(panelRows(page)).toHaveCount(10);
  // Fields changed without pressing Search.
  await panel(page).getByLabel('Talkgroup').selectOption({ index: 1 });
  await panel(page).getByLabel('From', { exact: true }).fill('2031-07-01T08:00');
  const turn = page.waitForRequest((request) => request.url().includes('/api/calls?') && request.url().includes('after='));
  await panel(page).getByRole('button', { name: 'Next' }).click();
  const page2 = new URL((await turn).url()).searchParams;
  expect(page2.get('q')).toBe('responding');
  expect(page2.get('talkgroups')).toBeNull();
  expect(page2.get('from')).toBeNull();
  const sheet = page.waitForRequest((request) => request.url().includes('/api/export/xlsx'));
  await panel(page).getByRole('button', { name: 'Excel — every match' }).click();
  const file = new URL((await sheet).url()).searchParams;
  expect(file.get('q')).toBe('responding');
  expect(file.get('talkgroups')).toBeNull();
  expect(file.get('from')).toBeNull();
});

test('a text saved from a panel row changes that row', async ({ page }) => {
  await page.goto('/?search=responding');
  const tr = panelRows(page).first();
  await expect(tr).toBeVisible();
  await tr.locator('details.row-actions summary').click();
  await tr.getByRole('button', { name: 'Edit text' }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit the text' });
  const text = `responding, corrected by a test ${Date.now()}`;
  await dialog.getByLabel('Text').fill(text);
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(dialog).toBeHidden();
  await expect(tr.locator('td.c-text')).toContainText(text);
});

/** A message is on screen, above whatever is under it (the panel too), clear of the player. */
const seen = async (page: Page, words: string) => {
  const notice = page.getByRole('status');
  await expect(notice).toContainText(words);
  const box = (await notice.boundingBox())!;
  const view = page.viewportSize()!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(view.width);
  expect(box.y + box.height).toBeLessThanOrEqual(view.height);
  expect(box.y + box.height).toBeLessThanOrEqual((await page.locator('footer.player').boundingBox())!.y);
  // The message lets clicks through; for this check it takes them, so the
  // element drawn on top at three of its points can be asked.
  await page.addStyleTag({ content: '.notice { pointer-events: auto !important; }' });
  const onTop = await notice.evaluate((el) => {
    const r = el.getBoundingClientRect();
    return [r.left + 4, r.left + r.width / 2, r.right - 4].map((x) => {
      const hit = document.elementFromPoint(x, r.top + r.height / 2);
      return hit === el || el.contains(hit);
    });
  });
  expect(onTop).toEqual([true, true, true]);
};

for (const width of [1280, 360]) {
  test(`a message is seen while the list is scrolled, and does not move its rows, at ${width} px`, async ({ page, context }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await page.setViewportSize({ width, height: 740 });
    await page.goto('/');
    await expect(rows(page)).toHaveCount(30);
    const tr = rows(page).nth(12);
    await tr.evaluate((row) => {
      const list = row.closest('main')!;
      list.scrollTop += row.getBoundingClientRect().top - list.getBoundingClientRect().top - 150;
    });
    expect(await page.locator('main.list').evaluate((list) => list.scrollTop)).toBeGreaterThan(0);
    const before = (await tr.boundingBox())!.y;
    // The menu is not opened: on a phone a row's menu opens out of sight once the list is scrolled.
    await tr.locator('button', { hasText: 'Share link' }).evaluate((item: HTMLButtonElement) => item.click());
    await seen(page, 'Link copied');
    expect((await tr.boundingBox())!.y).toBeCloseTo(before, 0);
  });

  test(`a message is seen above the search panel at ${width} px`, async ({ page, context }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await page.setViewportSize({ width, height: 740 });
    await page.goto('/?search=responding');
    const tr = panelRows(page).first();
    await expect(tr).toBeVisible();
    await tr.locator('details.row-actions summary').click();
    await tr.getByRole('button', { name: 'Share link' }).click();
    await seen(page, 'Link copied');
  });
}

/** Every item of the open menu is on screen, above the player and drawn on top. */
const onScreen = async (page: Page, items: Locator) => {
  const view = page.viewportSize()!;
  const floor = (await page.locator('footer.player').boundingBox())!.y;
  const count = await items.count();
  expect(count).toBeGreaterThan(0);
  for (let i = 0; i < count; i++) {
    const item = items.nth(i);
    await expect(item).toBeVisible();
    const box = (await item.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.y).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(view.width);
    expect(box.y + box.height).toBeLessThanOrEqual(floor);
    expect(
      await item.evaluate((el) => {
        const r = el.getBoundingClientRect();
        const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return hit === el || el.contains(hit);
      }),
    ).toBe(true);
  }
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(view.width);
};

test('on a phone a row\'s menu opens on screen, however far the list is scrolled', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await page.locator('main.list').evaluate((list) => (list.scrollTop = list.scrollHeight));
  const tr = rows(page).last();
  await tr.locator('details.row-actions summary').click();
  await onScreen(page, tr.locator('.menu button'));
  await tr.getByRole('button', { name: 'Edit text' }).click();
  await expect(page.getByRole('dialog', { name: 'Edit the text' })).toBeVisible();
});

test('on a phone the Export and Columns menus open on screen', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await exportMenu(page).locator('summary').click();
  await onScreen(page, exportMenu(page).locator('.menu button'));
  await page.keyboard.press('Escape');
  await page.locator('details.columns summary').click();
  await onScreen(page, page.locator('details.columns .menu input'));
});

/** The element is drawn at its own centre, so a listener can see it and reach it: not clipped, not off the screen. */
const reachable = (item: Locator) =>
  item.evaluate((el) => {
    const r = el.getBoundingClientRect();
    const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return hit === el || el.contains(hit);
  });

// A short screen, or a page zoomed in: the panel scrolls (with the wheel, as a
// listener does), so its pager and its player can still be reached.
for (const [width, height] of [[640, 360], [800, 450], [360, 640]]) {
  test(`the search panel scrolls to its pager and its player at ${width} x ${height}`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await page.goto('/?search=responding');
    await expect(panelRows(page).first()).toBeVisible();
    await panel(page).getByRole('heading').hover();
    await page.mouse.wheel(0, 5000);
    await expect
      .poll(() => panel(page).evaluate((el) => el.scrollHeight - el.clientHeight - el.scrollTop <= 1), { timeout: 2000 })
      .toBe(true);
    for (const item of [panel(page).getByLabel('Rows'), panel(page).getByRole('button', { name: 'Next' }), panel(page).locator('audio'), panel(page).locator('.now')]) {
      expect(await reachable(item)).toBe(true);
    }
    // Two pages of 10 calls; the second is read by Next.
    await panel(page).getByLabel('Rows').selectOption('10');
    await expect(panelRows(page)).toHaveCount(10);
    const first = await panelRows(page).first().getAttribute('data-id');
    await panel(page).getByRole('button', { name: 'Next' }).click();
    await expect(panel(page)).toContainText(/Page 2\b/);
    await expect(panelRows(page)).not.toHaveCount(0);
    expect(await panelRows(page).first().getAttribute('data-id')).not.toBe(first);
  });
}

/** WCAG 2.x contrast ratio of two `rgb(...)` or `rgba(...)` colours (alpha ignored). */
const contrast = (a: string, b: string) => {
  const luminance = (css: string) => {
    const [r, g, bl] = (css.match(/[\d.]+/g) ?? []).slice(0, 3).map((v) => {
      const c = Number(v) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};

for (const theme of ['light', 'dark']) {
  test(`the search panel's player has a focus ring of 3:1 or more in the ${theme} theme`, async ({ page }) => {
    await page.addInitScript((t) => localStorage.setItem('scannerpulse.theme', t), theme);
    await page.goto('/?search=responding');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expect(panelRows(page).first()).toBeVisible();
    const result = panel(page).locator('audio');
    await page.keyboard.press('Tab');
    await result.focus();
    expect(await result.evaluate((el) => el.matches(':focus-visible'))).toBe(true);
    const ring = await result.evaluate((el) => getComputedStyle(el).outlineColor);
    const ground = await panel(page).evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(contrast(ring, ground)).toBeGreaterThanOrEqual(3);
  });
}

test('an open search panel is left out of a printout', async ({ page }) => {
  await page.goto('/?search=responding');
  await expect(panelRows(page).first()).toBeVisible();
  await page.emulateMedia({ media: 'print' });
  await expect(page.locator('dialog.panel')).toBeHidden();
  await page.emulateMedia({ media: 'screen' });
  await expect(panel(page)).toBeVisible();
});
