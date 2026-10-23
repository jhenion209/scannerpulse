// Who is signed in on the admin page, and the admin API's requests. Every
// change carries the session's CSRF token; an answer of 401 means the session
// has ended, which brings the sign-in form back.

import { apiError } from '../api';

export const session = $state({
  /** The first read of who is signed in is done: until then neither the form nor the areas show. */
  checked: false,
  login: null as string | null,
  csrf: '',
  /** The session ended while the page was open (it expired, or was ended elsewhere). */
  ended: false,
  /** `super` may change everything; an `associate` only the functions on. */
  role: 'associate' as 'super' | 'associate',
  /** The functions the admin may use, by name: every one for a super admin. */
  functions: [] as string[],
});

/** Who the server says is signed in, and what they may change. */
export interface Me {
  login: string;
  csrf: string;
  role?: 'super' | 'associate';
  functions?: string[];
}

const JSON_ONLY = { accept: 'application/json' };

function signedIn(me: Me) {
  session.login = me.login;
  session.csrf = me.csrf;
  session.ended = false;
  session.role = me.role ?? 'associate';
  session.functions = me.functions ?? [];
}

/** Whether the signed-in admin may use the function (`move`, `purge`…):
 * the page shows a change only to an admin the server would let make it. */
export const can = (fn: string) => session.functions.includes(fn);

/** Whether the signed-in admin is a super admin. */
export const isSuper = () => session.role === 'super';

/** Reads who is signed in; throws only when the server cannot say. */
export async function checkSession(): Promise<void> {
  const response = await fetch('/api/auth/me', { headers: JSON_ONLY });
  if (response.ok) signedIn(await response.json());
  else if (response.status !== 401) throw await apiError(response);
  session.checked = true;
}

/** Signs in with a password; throws the server's refusal. */
export async function signIn(login: string, password: string): Promise<void> {
  const response = await fetch('/api/auth/login', {
    method: 'POST',
    headers: { ...JSON_ONLY, 'content-type': 'application/json' },
    body: JSON.stringify({ login, password }),
  });
  if (!response.ok) throw await apiError(response);
  signedIn(await response.json());
}

/** Ends the session. */
export async function signOut(): Promise<void> {
  const response = await fetch('/api/auth/logout', { method: 'POST' });
  if (!response.ok) throw await apiError(response);
  session.login = null;
  session.csrf = '';
  session.functions = [];
}

/** An admin request, JSON both ways; the answer's body, or undefined when it
 * has none. A change carries the CSRF token; refused for its token (a sign-in
 * in another tab gave the browser a new session), it reads the session again
 * and, if it is the same admin's, tries once more; another admin's session
 * refuses the change. A read the page makes on its own (`poll`) says so, and
 * the server does not count it as use of the session, so a page left open
 * cannot keep the session from ending. Throws the server's refusal. */
export async function admin<T = undefined>(
  method: 'GET' | 'POST' | 'PUT' | 'DELETE',
  path: string,
  body?: unknown,
  options?: { poll?: boolean },
): Promise<T> {
  const headers: Record<string, string> = { ...JSON_ONLY };
  if (method !== 'GET') headers['x-csrf-token'] = session.csrf;
  else if (options?.poll) headers['x-scannerpulse-poll'] = '1';
  if (body !== undefined) headers['content-type'] = 'application/json';
  let response = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  if (response.status === 403 && method !== 'GET') {
    const me = await fetch('/api/auth/me', { headers: JSON_ONLY });
    if (me.ok) {
      const fresh: Me = await me.json();
      // Another admin signed in on this browser: the change is not made in their name.
      if (fresh.login !== session.login) throw new Error('another admin signed in on this browser; sign in again');
      // A refusal may come from a function a super admin turned off: the
      // page then stops offering it.
      signedIn(fresh);
      if (fresh.csrf !== headers['x-csrf-token']) {
        headers['x-csrf-token'] = fresh.csrf;
        response = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
      }
    }
  }
  if (response.status === 401 && session.login !== null) {
    session.login = null;
    session.csrf = '';
    session.ended = true;
  }
  if (!response.ok) throw await apiError(response);
  const text = await response.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

/** An error's text, in the server's words when it has some. */
export const why = (error: unknown) => (error instanceof Error ? error.message : String(error));
