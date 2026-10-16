import { describe, expect, it } from 'vitest';
import { apiError } from './api';

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
