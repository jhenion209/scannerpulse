import { afterEach, describe, expect, it, vi } from 'vitest';
import { NO_FILTERS } from './filters';
import { FIRST_PAGE } from './paging';
import { goTo, show, view } from './calls.svelte';

const call = (id: number) => ({ id, started_at: '2026-10-15T12:00:00Z', duration_ms: 1000, talkgroup_id: 1, talkgroup: 101,
  talkgroup_name: 'Fire Dispatch', system: 'SYSA', site: null, radio_id: null, radio_alias: null, transcript: null, origin: null });

/** A server of 3 pages of 2 calls: page i's cursor is `c<i>`. */
function serve() {
  const asked: URLSearchParams[] = [];
  vi.stubGlobal('fetch', async (url: string) => {
    const query = new URLSearchParams(url.split('?')[1]);
    asked.push(query);
    const i = query.get('after') ? Number(query.get('after')!.slice(1)) : 0;
    const body = {
      calls: [call(i * 2 + 1), call(i * 2 + 2)],
      next: i < 2 ? `c${i + 1}` : null,
      total: query.get('count') === 'false' ? null : 6,
      total_capped: false,
    };
    return new Response(JSON.stringify(body), { status: 200 });
  });
  return asked;
}

const ask = { filters: { ...NO_FILTERS, allDates: true }, favorites: [], size: 10 };

afterEach(() => vi.unstubAllGlobals());

describe('the call list', () => {
  it('reads the first page with its count and keeps the next cursor', async () => {
    const asked = serve();
    await show(ask, FIRST_PAGE);
    expect(view.calls.map((c) => c.id)).toEqual([1, 2]);
    expect(view.total).toBe(6);
    expect(view.paging).toEqual({ cursors: [null, 'c1'], index: 0 });
    expect(asked[0].get('count')).toBeNull();
  });

  it('turns a page without counting again, keeping the total', async () => {
    const asked = serve();
    await show(ask, FIRST_PAGE);
    await show(ask, { ...view.paging, index: 1 }, false);
    expect(view.calls.map((c) => c.id)).toEqual([3, 4]);
    expect(view.total).toBe(6);
    expect(asked[1].get('count')).toBe('false');
    expect(asked[1].get('after')).toBe('c1');
  });

  it('jumps to a page by reading the ones between without counting', async () => {
    const asked = serve();
    await show(ask, FIRST_PAGE);
    await goTo(ask, 2);
    expect(view.calls.map((c) => c.id)).toEqual([5, 6]);
    expect(view.paging.index).toBe(2);
    expect(asked.slice(1).every((q) => q.get('count') === 'false')).toBe(true);
  });

  it('says why a page could not be read and keeps the rows it had', async () => {
    serve();
    await show(ask, FIRST_PAGE);
    vi.stubGlobal('fetch', async () =>
      new Response(JSON.stringify({ error: 'bad request', detail: 'size is one of [10]' }), { status: 400 }));
    await show(ask, { ...view.paging, index: 1 }, false);
    expect(view.error).toBe('size is one of [10]');
    expect(view.calls.map((c) => c.id)).toEqual([1, 2]);
  });
});
