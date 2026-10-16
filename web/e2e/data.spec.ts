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
  await answered;
  await expect(page.getByRole('status').filter({ hasText: 'calls of' })).toHaveCount(0);
  await expect(page.getByLabel(/^Type .* to confirm$/)).toHaveCount(0);
});
