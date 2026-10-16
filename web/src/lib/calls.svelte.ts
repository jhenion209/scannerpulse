import { fetchCalls, type Call, type Favorite } from './api';
import { listQuery, type Filters } from './filters';
import { FIRST_PAGE, walkTo, type Paging } from './paging';

/** What a list reads by. */
export interface Ask {
  filters: Filters;
  favorites: Favorite[];
  size: number;
  /** Search words (the search panel's). */
  text?: string;
  /** Count every match on a new read, not up to the server's cap. */
  exact?: boolean;
}

export type View = {
  calls: Call[];
  total: number | null;
  capped: boolean;
  paging: Paging;
  loading: boolean;
  error: string | null;
};

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** The request for one page of `ask`. */
function queryOf(ask: Ask, after: string | null, count: boolean): URLSearchParams {
  const query = listQuery(ask.filters, ask.favorites, { size: ask.size, after, count }, new Date());
  if (ask.text?.trim()) query.set('q', ask.text.trim());
  if (ask.exact && count) query.set('exact', 'true');
  return query;
}

/** A list of calls read page by page. Each list cancels only its own older
 * reads: the main list and the search panel's never stop each other. */
export function createList() {
  const view = $state<View>({
    calls: [],
    total: null,
    capped: false,
    paging: FIRST_PAGE,
    loading: false,
    error: null,
  });
  let current: AbortController | null = null;

  /** Reads page `paging.index` and returns false when a newer read replaced it.
   * `count` is false on a page turn, which keeps the total already shown. A
   * newer read cancels an older one. A read of a new list (the `FIRST_PAGE`
   * object itself) forgets the old cursors and total at once, so no page turn
   * can pair them with the new filters. */
  async function show(ask: Ask, paging: Paging, count = true): Promise<boolean> {
    current?.abort();
    const controller = (current = new AbortController());
    const fresh = paging === FIRST_PAGE;
    if (fresh) {
      view.paging = FIRST_PAGE;
      view.total = null;
      view.capped = false;
    }
    view.loading = true;
    try {
      const after = paging.cursors[paging.index];
      const page = await fetchCalls(queryOf(ask, after, count), controller.signal);
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
      if (!controller.signal.aborted) {
        view.error = message(error);
        if (fresh) view.calls = [];
      }
    } finally {
      if (current === controller) view.loading = false;
    }
    return !controller.signal.aborted;
  }

  /** Goes to page `target` (from 0), reading the pages before it when their
   * cursors are not known yet. It is a read like any other: it supersedes the
   * current one, and a newer read supersedes it, leaving no error behind; it
   * returns false when a newer read replaced it. */
  async function goTo(ask: Ask, target: number): Promise<boolean> {
    current?.abort();
    const controller = (current = new AbortController());
    view.loading = true;
    try {
      const paging = await walkTo(view.paging, target, async (after) =>
        (await fetchCalls(queryOf(ask, after, false), controller.signal)).next,
      );
      if (controller.signal.aborted) return false;
      return await show(ask, paging, false);
    } catch (error) {
      if (controller.signal.aborted) return false;
      view.error = message(error);
      return true;
    } finally {
      if (current === controller) view.loading = false;
    }
  }

  return { view, show, goTo };
}

/** The main page's list, shared by the list, the player and the live feed. */
const main = createList();
export const view = main.view;
export const show = main.show;
export const goTo = main.goTo;
