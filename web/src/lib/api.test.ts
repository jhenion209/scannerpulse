import { afterEach, describe, expect, it, vi } from 'vitest';
import { apiError, saveTranscript } from './api';

describe('the error an answer carries', () => {
  it('speaks in the server\'s words: the detail, else the error', async () => {
    const refused = new Response(JSON.stringify({ error: 'bad_text', detail: 'The text has a “<”.' }), { status: 422 });
    expect((await apiError(refused)).message).toBe('The text has a “<”.');
    const plain = new Response(JSON.stringify({ error: 'not_found' }), { status: 404 });
    expect((await apiError(plain)).message).toBe('not_found');
  });

  it('names the HTTP status when the answer has no JSON and no status text (a proxy\'s 502 over HTTP/2)', async () => {
    const proxy = new Response('<html>Bad gateway</html>', { status: 502, statusText: '' });
    const error = await apiError(proxy);
    expect(error.status).toBe(502);
    expect(error.message).toBe('HTTP 502');
  });

  it('keeps the status text when there is one', async () => {
    const proxy = new Response('gateway', { status: 504, statusText: 'Gateway Timeout' });
    expect((await apiError(proxy)).message).toBe('Gateway Timeout');
  });
});

describe('a saved text', () => {
  const saved = () => new Response(JSON.stringify({ id: 3, transcript: 'x', origin: 'human' }), { status: 200 });
  afterEach(() => vi.unstubAllGlobals());

  it("carries a signed-in admin's token, so it is saved under their name", async () => {
    const fetched = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(new Response(JSON.stringify({ login: 'ann', csrf: 'tok' }), { status: 200 }))
      .mockResolvedValueOnce(saved());
    vi.stubGlobal('fetch', fetched);
    await saveTranscript(3, 'x');
    expect(fetched.mock.calls[0][0]).toBe('/api/auth/me');
    expect((fetched.mock.calls[1][1]?.headers as Record<string, string>)['x-csrf-token']).toBe('tok');
  });

  it("is a visitor's when nobody is signed in", async () => {
    const fetched = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(new Response(JSON.stringify({ error: 'unauthorized', detail: '' }), { status: 401 }))
      .mockResolvedValueOnce(saved());
    vi.stubGlobal('fetch', fetched);
    await saveTranscript(3, 'x');
    expect((fetched.mock.calls[1][1]?.headers as Record<string, string>)['x-csrf-token']).toBeUndefined();
  });
});
