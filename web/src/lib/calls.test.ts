import { afterEach, describe, expect, it, vi } from 'vitest';
import { NO_FILTERS } from './filters';
import { FIRST_PAGE } from './paging';
import { createList, goTo, show, view } from './calls.svelte';

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

/** A server whose answers wait until the test releases them. A request
 * whose signal aborts rejects, as fetch does. */
function held() {
  const waiting: { query: URLSearchParams; answer: (body: object) => void }[] = [];
  vi.stubGlobal('fetch', (url: string, init?: RequestInit) => new Promise((resolve, reject) => {
    waiting.push({
      query: new URLSearchParams(url.split('?')[1]),
      answer: (body) => resolve(new Response(JSON.stringify(body), { status: 200 })),
    });
    init?.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
  }));
  return waiting;
}

const body = (ids: number[], next: string | null, total: number | null) => ({
  calls: ids.map(call), next, total, total_capped: false,
});

const ask = { filters: { ...NO_FILTERS, allDates: true }, favorites: [], size: 10, zone: 'UTC' };

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

  it('forgets the old cursors and total as a new list starts reading', async () => {
    serve();
    await show(ask, FIRST_PAGE);
    const waiting = held();
    const read = show(ask, FIRST_PAGE);
    expect(view.paging).toEqual({ cursors: [null], index: 0 });
    expect(view.total).toBeNull();
    waiting[0].answer(body([7], null, 1));
    await read;
    expect(view.calls.map((c) => c.id)).toEqual([7]);
    expect(view.total).toBe(1);
  });

  it('leaves no rows when a new list cannot be read', async () => {
    serve();
    await show(ask, FIRST_PAGE);
    vi.stubGlobal('fetch', async () =>
      new Response(JSON.stringify({ error: 'bad request', detail: 'no such talkgroup' }), { status: 400 }));
    await show(ask, FIRST_PAGE);
    expect(view.calls).toEqual([]);
    expect(view.error).toBe('no such talkgroup');
  });

  it('a jump superseded by a new list does not overwrite it or report an error', async () => {
    serve();
    await show(ask, FIRST_PAGE);
    const waiting = held();
    const jump = goTo(ask, 2);
    expect(waiting).toHaveLength(1);
    const fresh = show(ask, FIRST_PAGE);
    waiting[1].answer(body([9, 10], null, 2));
    await Promise.all([jump, fresh]);
    expect(view.calls.map((c) => c.id)).toEqual([9, 10]);
    expect(view.error).toBeNull();
    expect(view.loading).toBe(false);
  });
});

describe('a second list (the search panel\'s)', () => {
  it('reads by its own words and counts exactly on a new search, never touching the main list', async () => {
    const asked = serve();
    await show(ask, FIRST_PAGE);
    const mainCalls = view.calls.map((c) => c.id);
    const search = createList();
    await search.show({ ...ask, text: 'engine 7', exact: true }, FIRST_PAGE);
    const query = asked[asked.length - 1];
    expect(query.get('q')).toBe('engine 7');
    expect(query.get('exact')).toBe('true');
    expect(search.view.total).toBe(6);
    expect(view.calls.map((c) => c.id)).toEqual(mainCalls);
    // A page turn does not count again, so it asks for no exact count.
    await search.show({ ...ask, text: 'engine 7', exact: true }, { ...search.view.paging, index: 1 }, false);
    const turn = asked[asked.length - 1];
    expect(turn.get('count')).toBe('false');
    expect(turn.get('exact')).toBeNull();
    expect(turn.get('q')).toBe('engine 7');
  });

  it('does not cancel the main list\'s read, nor the main list its', async () => {
    const waiting = held();
    const search = createList();
    const main = show(ask, FIRST_PAGE);
    const other = search.show({ ...ask, text: 'x' }, FIRST_PAGE);
    expect(waiting).toHaveLength(2);
    waiting[0].answer(body([1], null, 1));
    waiting[1].answer(body([2], null, 1));
    expect(await main).toBe(true);
    expect(await other).toBe(true);
    expect(view.calls.map((c) => c.id)).toEqual([1]);
    expect(search.view.calls.map((c) => c.id)).toEqual([2]);
  });
});
