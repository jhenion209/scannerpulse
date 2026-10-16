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
});

const JSON_ONLY = { accept: 'application/json' };

function signedIn(me: { login: string; csrf: string }) {
  session.login = me.login;
  session.csrf = me.csrf;
  session.ended = false;
}

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
}

/** An admin request, JSON both ways; the answer's body, or undefined when it
 * has none. A change carries the CSRF token. Throws the server's refusal. */
export async function admin<T = undefined>(
  method: 'GET' | 'POST' | 'PUT' | 'DELETE',
  path: string,
  body?: unknown,
): Promise<T> {
  const headers: Record<string, string> = { ...JSON_ONLY };
  if (method !== 'GET') headers['x-csrf-token'] = session.csrf;
  if (body !== undefined) headers['content-type'] = 'application/json';
  const response = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
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
