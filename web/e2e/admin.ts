import { expect, type Page } from '@playwright/test';

// What the admin page's tests share: the admin e2e/seed.sh makes, and
// signing in as them.

export const ADMIN = { login: 'e2e-admin', password: 'e2e admin password' };

/** Opens the admin page (at `hash`, such as `#status`) and signs in. */
export async function signIn(page: Page, hash = '') {
  await page.goto(`/admin${hash}`);
  await page.getByLabel('Name').fill(ADMIN.login);
  await page.getByLabel('Password').fill(ADMIN.password);
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
