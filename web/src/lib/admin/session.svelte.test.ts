import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { admin, checkSession, session, signIn, signOut } from './session.svelte';

const answer = (status: number, body?: unknown) =>
  new Response(body === undefined ? null : JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
const fetched = vi.fn<typeof fetch>();

beforeEach(() => {
  vi.stubGlobal('fetch', fetched);
  fetched.mockReset();
  Object.assign(session, { checked: false, login: null, csrf: '', ended: false });
});
afterEach(() => vi.unstubAllGlobals());

const sent = (n = 0) => {
  const [path, init] = fetched.mock.calls[n];
  return { path, method: init?.method ?? 'GET', headers: (init?.headers ?? {}) as Record<string, string>, body: init?.body };
};

describe('signing in', () => {
  it('knows who is signed in, or that nobody is', async () => {
    fetched.mockResolvedValueOnce(answer(200, { login: 'ann', csrf: 't1' }));
    await checkSession();
    expect(session).toMatchObject({ checked: true, login: 'ann', csrf: 't1' });
    fetched.mockResolvedValueOnce(answer(401, { error: 'unauthorized', detail: 'sign in as an admin first' }));
    session.login = null;
    await checkSession();
    expect(session).toMatchObject({ checked: true, login: null });
  });

  it('throws when the server cannot say who is signed in', async () => {
    fetched.mockResolvedValueOnce(answer(503, { error: 'service unavailable', detail: 'the database does not answer' }));
    await expect(checkSession()).rejects.toThrow('the database does not answer');
    expect(session.checked).toBe(false);
  });

  it("gives a refused sign-in in the server's words", async () => {
    fetched.mockResolvedValueOnce(answer(401, { error: 'unauthorized', detail: 'the name or the password is wrong' }));
    await expect(signIn('ann', 'nope')).rejects.toThrow('the name or the password is wrong');
    expect(session.login).toBeNull();
    fetched.mockResolvedValueOnce(answer(200, { login: 'ann', csrf: 't2' }));
    await signIn('ann', 'right password');
    expect(sent(1)).toMatchObject({ path: '/api/auth/login', method: 'POST', body: JSON.stringify({ login: 'ann', password: 'right password' }) });
    expect(session).toMatchObject({ login: 'ann', csrf: 't2' });
  });

  it('forgets the session on signing out', async () => {
    Object.assign(session, { login: 'ann', csrf: 't3' });
    fetched.mockResolvedValueOnce(answer(204));
    await signOut();
    expect(sent()).toMatchObject({ path: '/api/auth/logout', method: 'POST' });
    expect(session).toMatchObject({ login: null, csrf: '' });
  });
});

describe('an admin request', () => {
  beforeEach(() => Object.assign(session, { checked: true, login: 'ann', csrf: 'tok' }));

  it('carries the token on a change, and not on a read', async () => {
    fetched.mockResolvedValueOnce(answer(200, [1]));
    expect(await admin<number[]>('GET', '/api/admin/jobs')).toEqual([1]);
    expect(sent(0).headers['x-csrf-token']).toBeUndefined();
    fetched.mockResolvedValueOnce(answer(204));
    expect(await admin('PUT', '/api/admin/settings/web.system_order', { value: ['B'] })).toBeUndefined();
    expect(sent(1).headers).toMatchObject({ 'x-csrf-token': 'tok', 'content-type': 'application/json' });
    expect(sent(1).body).toBe(JSON.stringify({ value: ['B'] }));
  });

  it('marks a timed read, and only a read', async () => {
    fetched.mockImplementation(async () => answer(200, [1]));
    await admin('GET', '/api/admin/jobs', undefined, { poll: true });
    expect(sent(0).headers['x-scannerpulse-poll']).toBe('1');
    await admin('GET', '/api/admin/jobs');
    expect(sent(1).headers['x-scannerpulse-poll']).toBeUndefined();
    await admin('POST', '/api/admin/rescan', undefined, { poll: true });
    expect(sent(2).headers['x-scannerpulse-poll']).toBeUndefined();
  });

  it("gives a refusal in the server's words", async () => {
    fetched.mockResolvedValueOnce(answer(409, { error: 'conflict', detail: 'the last enabled admin cannot be disabled' }));
    await expect(admin('POST', '/api/admin/accounts/ann/disable')).rejects.toThrow('the last enabled admin cannot be disabled');
    expect(session.login).toBe('ann');
  });

  it('reads the session again and tries once more when a change is refused for its token', async () => {
    fetched
      .mockResolvedValueOnce(answer(403, { error: 'forbidden', detail: "a change needs the session's x-csrf-token header" }))
      .mockResolvedValueOnce(answer(200, { login: 'ann', csrf: 'new' }))
      .mockResolvedValueOnce(answer(204));
    await admin('PUT', '/api/admin/settings/web.system_order', { value: ['B'] });
    expect(sent(1).path).toBe('/api/auth/me');
    expect(sent(2).headers['x-csrf-token']).toBe('new');
    expect(session).toMatchObject({ login: 'ann', csrf: 'new' });
  });

  it('does not send a change again when another admin has signed in on this browser', async () => {
    fetched
      .mockResolvedValueOnce(answer(403, { error: 'forbidden', detail: "a change needs the session's x-csrf-token header" }))
      .mockResolvedValueOnce(answer(200, { login: 'bob', csrf: 'new' }));
    await expect(admin('PUT', '/api/admin/settings/web.system_order', { value: ['B'] }))
      .rejects.toThrow('another admin signed in on this browser; sign in again');
    expect(fetched).toHaveBeenCalledTimes(2);
    // The page does not take the other admin's place: a change is never made in their name.
    expect(session).toMatchObject({ login: 'ann', csrf: 'tok' });
  });

  it('gives a refusal that is not about the token in the server words, without trying again', async () => {
    fetched
      .mockResolvedValueOnce(answer(403, { error: 'forbidden', detail: 'password sign-in is off' }))
      .mockResolvedValueOnce(answer(200, { login: 'ann', csrf: 'tok' }));
    await expect(admin('POST', '/api/admin/rescan')).rejects.toThrow('password sign-in is off');
    expect(fetched).toHaveBeenCalledTimes(2);
  });

  it('brings the sign-in form back when the session has ended', async () => {
    fetched.mockResolvedValueOnce(answer(401, { error: 'unauthorized', detail: 'sign in as an admin first' }));
    await expect(admin('GET', '/api/admin/status')).rejects.toThrow('sign in as an admin first');
    expect(session).toMatchObject({ login: null, csrf: '', ended: true });
  });
});
