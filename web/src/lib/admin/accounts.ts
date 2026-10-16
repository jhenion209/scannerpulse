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
}

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
