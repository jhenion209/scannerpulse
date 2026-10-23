// The admin accounts (`/api/admin/accounts`) as the Accounts area shows them.

export interface Account {
  login: string;
  /** It can sign in with a password. */
  password: boolean;
  /** It signs in through the OIDC provider. */
  oidc: boolean;
  disabled: boolean;
  created_at: string;
  last_sign_in: string | null;
  role: 'super' | 'associate';
  /** The functions on, by name: what an associate may change. */
  functions: string[];
}

/** A function a super admin can turn on for an associate. */
export interface Fn {
  name: string;
  group: string;
  label: string;
}

/** The answer of `/api/admin/accounts`. */
export interface Accounts {
  accounts: Account[];
  functions: Fn[];
}

/** The functions in their groups, each group once, in the order given. */
export function grouped(functions: Fn[]): { group: string; functions: Fn[] }[] {
  const groups: { group: string; functions: Fn[] }[] = [];
  for (const fn of functions) {
    const last = groups.find((g) => g.group === fn.group);
    if (last) last.functions.push(fn);
    else groups.push({ group: fn.group, functions: [fn] });
  }
  return groups;
}

/** The role in words. */
export const roleWords = (account: Account) => (account.role === 'super' ? 'super admin' : 'associate');

/** How the account signs in, in words. */
export function signsInWith(account: Account): string {
  if (account.password && account.oidc) return 'a password or single sign-on';
  if (account.oidc) return 'single sign-on';
  if (account.password) return 'a password';
  return 'nothing yet';
}

/** The two typed passwords must be the same; the server checks the rest. */
export const passwordProblem = (first: string, again: string) =>
  first !== again ? 'The two passwords differ.' : null;

/** The settings of signing in that the Accounts area shows beside the accounts. */
export const SIGN_IN_KEYS = ['auth.password_signin', 'oidc.issuer', 'oidc.client_id', 'oidc.admin_group', 'oidc.redirect_url'];
