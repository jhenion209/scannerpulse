import { expect, test, type Page } from '@playwright/test';
import { freshStorage, signIn } from './admin';

// The clocks to look at, on Status, the Layout area and the Recorders area.
// No clock is checked beside these tests: the server's own answers are
// given clocks to name.

test.beforeEach(async ({ page }) => freshStorage(page));

/** Answers `path` with the server's answer, changed by `change`. */
const changed = <T>(page: Page, path: string, change: (body: T) => void) =>
  page.route(path, async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as T;
    change(body);
    await route.fulfill({ response, json: body });
  });

test('Status names each clock to look at', async ({ page }) => {
  await changed<{ clocks?: { subject: string; text: string }[] }>(page, '**/api/admin/status', (body) => {
    body.clocks = [
      { subject: 'db2', text: 'its clock is not synchronized by NTP' },
      { subject: 'live', text: "its clock is 2.0 s ahead of east's (12 copies): one call can be kept as two" },
    ];
  });
  await signIn(page);
  await expect(page.getByRole('alert').filter({ hasText: 'db2: its clock is not synchronized by NTP.' })).toBeVisible();
  await expect(
    page.getByRole('alert').filter({ hasText: "live: its clock is 2.0 s ahead of east's (12 copies): one call can be kept as two." }),
  ).toBeVisible();
});

test("the Layout area flags a machine's clock on its row", async ({ page }) => {
  await changed<{ machines: { clock?: string }[] }>(page, '**/api/admin/layout', (body) => {
    body.machines[0].clock = "its clock is 2.4 s behind the database's";
  });
  await signIn(page, '#layout');
  await expect(page.getByText("its clock is 2.4 s behind the database's")).toBeVisible();
});

test("the Recorders area says what is wrong with a recorder's clock on its card", async ({ page }) => {
  const text = 'its files are seen 90.0 s after their calls end: its clock may be behind, or its files slow to arrive';
  await changed<{ recorders: { name: string; clock?: string }[] }>(page, '**/api/admin/recorders', (body) => {
    body.recorders.find((r) => r.name === 'live')!.clock = text;
  });
  await signIn(page, '#recorders');
  const card = page.getByRole('listitem').filter({ has: page.getByRole('heading', { name: /^live / }) });
  await expect(card).toContainText(
    'Its files are seen 90.0 s after their calls end: its clock may be behind, or its files slow to arrive.',
  );
});
