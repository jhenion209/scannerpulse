import { expect, test, type Page } from '@playwright/test';

// The status chips when `/api/status` fails (the server answers 503 with a
// JSON body when its read is too slow). Against e2e/seed.sql, as main.spec.ts.

const rows = (page: Page) => page.locator('tbody tr');
const down = { status: 503, json: { error: 'unavailable', detail: 'too slow' } };

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('e2e')) {
      localStorage.clear();
      sessionStorage.setItem('e2e', '1');
    }
  });
});

for (const width of [1280, 360]) {
  test(`a status that cannot be read says so beside a list that loads, at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.route('**/api/status', (route) => route.fulfill(down));
    await page.goto('/');
    await expect(rows(page)).toHaveCount(30);
    await expect(page.getByRole('list', { name: 'Status' })).toContainText('Status unavailable');
    await expect(page.getByText('Status unavailable')).toBeVisible();
  });
}

for (const width of [1280, 360]) {
  test(`a status read that fails later keeps the last figures and adds the warning, at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.clock.install();
    let failing = false;
    await page.route('**/api/status', async (route) => {
      if (failing) return route.fulfill(down);
      // As on a newly seeded database: the newest call is as new as the server's clock.
      const response = await route.fetch();
      const body = await response.json();
      body.newest_call = body.server_time;
      await route.fulfill({ response, json: body });
    });
    await page.goto('/');
    await expect(rows(page)).toHaveCount(30);
    // A phone hides the whole list while the status is healthy and the newest call fresh.
    const chips = page.getByRole('list', { name: 'Status', includeHidden: true });
    await expect(chips).toContainText('35 calls');
    await expect(chips).not.toContainText('Status unavailable');
    if (width < 760) await expect(chips).toBeHidden();
    else await expect(chips).toBeVisible();

    failing = true;
    await page.clock.fastForward('00:31');
    await expect(chips).toContainText('Status unavailable');
    await expect(chips).toContainText('35 calls');
    await expect(page.getByText('Status unavailable')).toBeVisible();
  });
}
