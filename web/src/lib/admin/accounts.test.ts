import { describe, expect, it } from 'vitest';
import { grouped, passwordProblem, roleWords, signsInWith, type Account } from './accounts';

const account = (over: Partial<Account>): Account => ({
  login: 'ann', password: true, oidc: false, disabled: false, created_at: '2031-07-01T10:00:00Z', last_sign_in: null,
  role: 'super', functions: [], ...over,
});

describe('an account', () => {
  it('says how it signs in', () => {
    expect(signsInWith(account({}))).toBe('a password');
    expect(signsInWith(account({ password: false, oidc: true }))).toBe('single sign-on');
    expect(signsInWith(account({ oidc: true }))).toBe('a password or single sign-on');
    expect(signsInWith(account({ password: false }))).toBe('nothing yet');
  });
});

describe('roles and functions', () => {
  it('names the role', () => {
    expect(roleWords(account({}))).toBe('super admin');
    expect(roleWords(account({ role: 'associate' }))).toBe('associate');
  });

  it('keeps each group once, in the order the server gives', () => {
    const fns = [
      { name: 'locations', group: 'Storage', label: 'Locations' },
      { name: 'rename', group: 'Talkgroups', label: 'Rename' },
      { name: 'move', group: 'Storage', label: 'Move' },
    ];
    expect(grouped(fns).map((g) => [g.group, g.functions.map((f) => f.name)])).toEqual([
      ['Storage', ['locations', 'move']],
      ['Talkgroups', ['rename']],
    ]);
  });
});

describe('a new password', () => {
  it('is typed twice the same', () => {
    expect(passwordProblem('a long password', 'a long password')).toBeNull();
    expect(passwordProblem('a long password', 'a long passwork')).toBe('The two passwords differ.');
  });
});
