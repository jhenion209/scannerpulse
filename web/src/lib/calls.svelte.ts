import { fetchCalls, type Call, type Favorite } from './api';
import { listQuery, type Filters } from './filters';
import { FIRST_PAGE, walkTo, type Paging } from './paging';

/** What the list reads by. */
export interface Ask {
  filters: Filters;
  favorites: Favorite[];
  size: number;
}

/** The page of calls on screen, shared by the list, the player and the live feed. */
export const view = $state({
  calls: [] as Call[],
  total: null as number | null,
  capped: false,
  paging: FIRST_PAGE as Paging,
  loading: false,
  error: null as string | null,
});

let current: AbortController | null = null;

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** Reads page `paging.index`; `count` is false on a page turn, which keeps
 * the total already shown. A newer read cancels an older one. */
export async function show(ask: Ask, paging: Paging, count = true): Promise<void> {
  current?.abort();
  const controller = (current = new AbortController());
  view.loading = true;
  try {
    const after = paging.cursors[paging.index];
    const page = await fetchCalls(
      listQuery(ask.filters, ask.favorites, { size: ask.size, after, count }, new Date()),
      controller.signal,
    );
    const cursors = paging.cursors.slice(0, paging.index + 1);
    if (page.next) cursors.push(page.next);
    view.calls = page.calls;
    view.paging = { cursors, index: paging.index };
    if (page.total !== null) {
      view.total = page.total;
      view.capped = page.total_capped;
    }
    view.error = null;
  } catch (error) {
    if (!controller.signal.aborted) view.error = message(error);
  } finally {
    if (current === controller) view.loading = false;
  }
}

/** Goes to page `target` (from 0), reading the pages before it when their
 * cursors are not known yet. */
export async function goTo(ask: Ask, target: number): Promise<void> {
  try {
    const paging = await walkTo(view.paging, target, async (after) => {
      const query = listQuery(ask.filters, ask.favorites, { size: ask.size, after, count: false }, new Date());
      return (await fetchCalls(query)).next;
    });
    await show(ask, paging, false);
  } catch (error) {
    view.error = message(error);
  }
}
