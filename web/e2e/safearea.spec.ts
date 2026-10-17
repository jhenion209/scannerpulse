import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// An iPhone on its side, 844 x 390: the Dynamic Island takes 59 px of one
// side, the home bar 21 px of the bottom. The bars' colour runs to the
// screen's edge; no text or control sits under the island or the home bar.

test.beforeEach(async ({ page }) => freshStorage(page));

const W = 844, H = 390, ISLAND = 59, HOME = 21;

async function onItsSide(page: Page, side: 'left' | 'right') {
  await page.setViewportSize({ width: W, height: H });
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Emulation.setSafeAreaInsetsOverride', {
    insets: { left: side === 'left' ? ISLAND : 0, right: side === 'right' ? ISLAND : 0, top: 0, bottom: HOME },
  });
}

/** The outermost edges of the text and controls inside `scope` that can be
 * seen: each clipped by its scrolling ancestors, and none off the screen
 * (the skip link waits off it until it has the focus). */
const edges = (page: Page, scope: string) =>
  page.evaluate((scope) => {
    let left = Infinity, right = -Infinity, bottom = -Infinity;
    for (const el of document.querySelectorAll(`${scope} :is(button, a, input, select, label, td, th, span, h1, h2, p, b, audio)`)) {
      if (getComputedStyle(el).visibility === 'hidden') continue;
      let { left: l, right: r, top: t, bottom: b } = el.getBoundingClientRect();
      for (let up = el.parentElement; up; up = up.parentElement) {
        const style = getComputedStyle(up);
        if (style.overflowX === 'visible' && style.overflowY === 'visible') continue;
        const box = up.getBoundingClientRect();
        l = Math.max(l, box.left + up.clientLeft);
        r = Math.min(r, box.left + up.clientLeft + up.clientWidth);
        t = Math.max(t, box.top + up.clientTop);
        b = Math.min(b, box.top + up.clientTop + up.clientHeight);
      }
      if (r - l < 1 || b - t < 1 || r <= 0 || l >= innerWidth) continue;
      left = Math.min(left, l);
      right = Math.max(right, r);
      bottom = Math.max(bottom, b);
    }
    return { left, right, bottom };
  }, scope);

for (const side of ['left', 'right'] as const) {
  test(`on a phone on its side, island ${side}, the main page keeps its text and controls clear`, async ({ page }) => {
    await onItsSide(page, side);
    await page.goto('/');
    await page.locator('#calls tbody tr').first().waitFor();
    const bar = await page.locator('header.bar').boundingBox();
    expect(bar!.x).toBe(0);
    expect(bar!.width).toBe(W);
    for (const scope of ['header.bar', '.rail', '#calls', 'footer.player']) {
      const e = await edges(page, scope);
      if (side === 'left') expect(e.left, scope).toBeGreaterThanOrEqual(ISLAND);
      else expect(e.right, scope).toBeLessThanOrEqual(W - ISLAND);
    }
    expect((await edges(page, 'footer.player')).bottom).toBeLessThanOrEqual(H - HOME);
    // With the filters closed, the list itself is at the edge.
    await page.getByRole('button', { name: 'Filters' }).click();
    const list = await edges(page, '#calls');
    if (side === 'left') expect(list.left).toBeGreaterThanOrEqual(ISLAND);
    else expect(list.right).toBeLessThanOrEqual(W - ISLAND);
  });

  test(`on a phone on its side, island ${side}, the admin page keeps its text and controls clear`, async ({ page }) => {
    await onItsSide(page, side);
    await signIn(page, '#status');
    await page.getByRole('heading', { name: 'Status', level: 1 }).waitFor();
    expect((await page.locator('header.bar').boundingBox())!.x).toBe(0);
    for (const scope of ['header.bar', 'nav.side', '.area']) {
      const e = await edges(page, scope);
      if (side === 'left') expect(e.left, scope).toBeGreaterThanOrEqual(ISLAND);
      else expect(e.right, scope).toBeLessThanOrEqual(W - ISLAND);
    }
  });
}

test('the pages let their colour run under the island', async ({ page }) => {
  for (const path of ['/', '/admin']) {
    await page.goto(path);
    await expect(page.locator('meta[name="viewport"]')).toHaveAttribute('content', /viewport-fit=cover/);
  }
});
