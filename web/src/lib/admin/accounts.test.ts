import { describe, expect, it } from 'vitest';
import { passwordProblem, signsInWith, type Account } from './accounts';

const account = (over: Partial<Account>): Account => ({
  login: 'ann', password: true, oidc: false, disabled: false, created_at: '2031-07-01T10:00:00Z', last_sign_in: null, ...over,
});

describe('an account', () => {
  it('says how it signs in', () => {
    expect(signsInWith(account({}))).toBe('a password');
    expect(signsInWith(account({ password: false, oidc: true }))).toBe('single sign-on');
    expect(signsInWith(account({ oidc: true }))).toBe('a password or single sign-on');
    expect(signsInWith(account({ password: false }))).toBe('nothing yet');
  });
});

describe('a new password', () => {
  it('is typed twice the same', () => {
    expect(passwordProblem('a long password', 'a long password')).toBeNull();
    expect(passwordProblem('a long password', 'a long passwork')).toBe('The two passwords differ.');
  });
});
