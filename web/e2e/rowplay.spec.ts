import { expect, test, type Locator, type Page } from '@playwright/test';

// A click anywhere on a call's row plays it, and the row's left edge says if
// the call was played. Against e2e/seed.sql: calls 1 to 30 in the last 90
// minutes (call 1 the newest, so row n is call n); the odd calls have text.

const rows = (page: Page) => page.locator('tbody tr');
const ids = (page: Page) => rows(page).evaluateAll((trs) => trs.map((tr) => Number((tr as HTMLElement).dataset.id)));
const playingRow = (page: Page) => page.locator('tr.playing');
const bar = (row: Locator) => row.locator('td.c-play button');
const panel = (page: Page) => page.getByRole('dialog', { name: 'Search the archive' });

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('e2e')) {
      localStorage.clear();
      sessionStorage.setItem('e2e', '1');
    }
  });
});

test("a click anywhere on a call's row plays it", async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const all = await ids(page);
  await rows(page).nth(2).locator('td.c-talkgroup').click();
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[2]));
  await rows(page).nth(4).locator('td.c-length').click();
  await expect(playingRow(page)).toHaveCount(1);
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[4]));
});

test('More, the menu and the selection box keep their own clicks', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const third = rows(page).nth(2);
  await third.getByRole('button', { name: 'More' }).click();
  await expect(third.getByRole('button', { name: 'Less' })).toHaveAttribute('aria-expanded', 'true');
  await expect(playingRow(page)).toHaveCount(0);
  await third.locator('details.row-actions summary').click();
  await expect(third.getByRole('button', { name: 'Edit text' })).toBeVisible();
  await expect(playingRow(page)).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(third.getByRole('button', { name: 'Edit text' })).toBeHidden();
  await page.getByRole('button', { name: 'Select calls' }).click();
  const second = rows(page).nth(1);
  await second.locator('td.c-select input').check();
  await expect(second).toHaveClass(/selected/);
  await expect(playingRow(page)).toHaveCount(0);
  // The row itself plays, in selection mode too, and stays chosen.
  await second.locator('td.c-talkgroup').click();
  await expect(playingRow(page)).toHaveAttribute('data-id', (await second.getAttribute('data-id'))!);
  await expect(second).toHaveClass(/selected/);
});

test("selecting some of a row's text does not play it", async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const box = (await rows(page).nth(2).locator('.snippet').boundingBox())!;
  const y = box.y + box.height / 2;
  await page.mouse.move(box.x + 4, y);
  await page.mouse.down();
  await page.mouse.move(box.x + 60, y, { steps: 6 });
  await page.mouse.move(box.x + 120, y, { steps: 6 });
  await page.mouse.up();
  expect(await page.evaluate(() => window.getSelection()?.toString() ?? '')).not.toBe('');
  await expect(playingRow(page)).toHaveCount(0);
});

test("a click on a search result's row plays it in the panel's player", async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await page.getByLabel('Words to search for').fill('Engine');
  await page.getByRole('button', { name: 'Search', exact: true }).click();
  // Call 9, not the first result: earlier tests edit calls 1 and 3, so the first
  // result can be call 7, which has no file (e2e/seed.sql) and cannot be played.
  const result = panel(page).locator('tbody tr', { hasText: 'Engine 9 responding' }).first();
  await expect(result).toBeVisible();
  await expect(panel(page)).toContainText('No result playing');
  await result.locator('td.c-talkgroup').click();
  await expect(result).toHaveClass(/playing/);
  await expect(panel(page)).not.toContainText('No result playing');
});

test('the left edge says whether a call was played, by width and colour', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  const all = await ids(page);
  const look = (row: Locator) =>
    bar(row).evaluate((el) => {
      const style = getComputedStyle(el);
      return { color: style.backgroundColor, width: parseFloat(style.width) };
    });
  const [third, fifth] = [rows(page).nth(2), rows(page).nth(4)];
  const unplayed = await look(third);
  await third.locator('td.c-talkgroup').click();
  await expect(third).toHaveClass(/playing/);
  // The playing call keeps the bar of a call not yet played.
  expect(await look(third)).toEqual(unplayed);
  await fifth.locator('td.c-talkgroup').click();
  await expect(fifth).toHaveClass(/playing/);
  await expect(third).toHaveClass(/played/);
  const played = await look(third);
  expect(played.color).not.toBe(unplayed.color);
  expect(played.width).toBeLessThan(unplayed.width);
  expect(await look(fifth)).toEqual(unplayed);
  // The bar is the keyboard's play button.
  await bar(third).focus();
  await page.keyboard.press('Enter');
  await expect(playingRow(page)).toHaveAttribute('data-id', String(all[2]));
});

/** The contrast of the bar over the cell it sits on, as WCAG relative luminance gives it. */
const contrast = (row: Locator) =>
  bar(row).evaluate((el) => {
    const px = (css: string) => {
      const c = document.createElement('canvas').getContext('2d', { willReadFrequently: true })!;
      c.fillStyle = css;
      c.fillRect(0, 0, 1, 1);
      return Array.from(c.getImageData(0, 0, 1, 1).data);
    };
    const lum = ([r, g, b]: number[]) =>
      [r, g, b].map((v) => (v / 255 <= 0.03928 ? v / 255 / 12.92 : ((v / 255 + 0.055) / 1.055) ** 2.4)).reduce((s, v, i) => s + v * [0.2126, 0.7152, 0.0722][i], 0);
    // The first background behind the bar that paints something.
    let behind: Element | null = el.parentElement;
    while (behind && px(getComputedStyle(behind).backgroundColor)[3] === 0) behind = behind.parentElement;
    const [a, b] = [lum(px(getComputedStyle(el).backgroundColor)), lum(px(getComputedStyle(behind!).backgroundColor))].sort((x, y) => y - x);
    return (a + 0.05) / (b + 0.05);
  });

test('the edge has enough contrast in both themes, and runs down the whole card on a phone', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  for (const theme of ['light', 'dark']) {
    await page.goto('/');
    await page.evaluate((t) => localStorage.setItem('scannerpulse.theme', t), theme);
    await page.reload();
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expect(rows(page)).toHaveCount(30);
    // Row 3 played, row 5 playing, the others not yet played.
    await rows(page).nth(2).locator('td.c-talkgroup').click();
    await rows(page).nth(4).locator('td.c-talkgroup').click();
    await expect(rows(page).nth(2)).toHaveClass(/played/);
    await expect(rows(page).nth(4)).toHaveClass(/playing/);
    const seen = [
      await contrast(rows(page).nth(0)),
      await contrast(rows(page).nth(2)),
      await contrast(rows(page).nth(4)),
    ];
    // And rows 1 and 3 chosen: the tint behind the bar.
    await page.getByRole('button', { name: 'Select calls' }).click();
    for (const n of [0, 2]) {
      await rows(page).nth(n).locator('td.c-select input').check();
      await expect(rows(page).nth(n)).toHaveClass(/selected/);
      seen.push(await contrast(rows(page).nth(n)));
    }
    for (const ratio of seen) expect(ratio, `${theme}: ${seen.map((r) => r.toFixed(2))}`).toBeGreaterThanOrEqual(3);
  }

  // On a phone each call is a card of several lines: the bar runs down all of them.
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  // Row 1 has text, so its card has two lines.
  for (const selecting of [false, true]) {
    if (selecting) await page.getByRole('button', { name: 'Select calls' }).click();
    const row = (await rows(page).first().boundingBox())!;
    const edge = (await bar(rows(page).first()).boundingBox())!;
    expect(edge.height).toBeGreaterThan(40);
    expect(Math.abs(edge.height - row.height)).toBeLessThanOrEqual(2);
    // It is the card's left edge, ahead of the box when there is one.
    expect(edge.x - row.x).toBeLessThanOrEqual(6);
    if (selecting) expect((await rows(page).first().locator('td.c-select input').boundingBox())!.x).toBeGreaterThan(edge.x + edge.width);
  }
});

test('the column headings stay over the rows, and their bars, as the list scrolls', async ({ page }) => {
  await page.goto('/');
  await expect(rows(page)).toHaveCount(30);
  await page.locator('.list').evaluate((list) => (list.scrollTop = 150));
  for (const heading of [page.locator('thead th').first(), page.getByRole('columnheader', { name: 'Talkgroup' })]) {
    const covered = await heading.evaluate((th) => {
      const box = th.getBoundingClientRect();
      const top = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
      return !(top && th.contains(top));
    });
    expect(covered, 'a row paints over the heading').toBe(false);
  }
});
