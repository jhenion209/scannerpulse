import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';

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
