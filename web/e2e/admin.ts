import { expect, type Page } from '@playwright/test';

// What the admin page's tests share: the admin e2e/seed.sh makes, and
// signing in as them.

export const ADMIN = { login: 'e2e-admin', password: 'e2e admin password' };
/** An associate who may only move a date range. */
export const ASSOCIATE = { login: 'e2e-associate', password: 'e2e associate password' };

/** Opens the admin page (at `hash`, such as `#status`) and signs in, as the
 * super admin unless told otherwise. */
export async function signIn(page: Page, hash = '', who = ADMIN) {
  await page.goto(`/admin${hash}`);
  await page.getByLabel('Name').fill(who.login);
  await page.getByLabel('Password').fill(who.password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('navigation', { name: 'Admin areas', includeHidden: true })).toBeAttached();
}

/** Starts each test with the browser's storage empty, as a new visitor. */
export async function freshStorage(page: Page) {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('e2e')) {
      localStorage.clear();
      sessionStorage.setItem('e2e', '1');
    }
  });
}
