// The API pages by cursor, not by number: page i is read with the cursor the
// page before it gave. The page keeps every cursor it has seen, so going back
// costs nothing, and a jump forward reads the pages in between, without
// counting, up to a limit.

export interface Paging {
  /** `cursors[i]` is the `after` that reads page i; page 0's is null. */
  cursors: (string | null)[];
  index: number;
}

export const FIRST_PAGE: Paging = { cursors: [null], index: 0 };

/** The most pages a jump reads to reach its page. */
export const MAX_WALK = 50;

export class PageTooFar extends Error {}

/** How many pages the total fills; `more` when the server stopped counting. */
export function pageCount(total: number, capped: boolean, size: number) {
  return { pages: Math.max(1, Math.ceil(total / size)), more: capped };
}

/** The paging at page `target`, reading forward with `next` (which answers a
 * page's `next` cursor, or null on the last page) when it is not known yet. */
export async function walkTo(
  paging: Paging,
  target: number,
  next: (after: string | null) => Promise<string | null>,
): Promise<Paging> {
  const cursors = [...paging.cursors];
  if (target - (cursors.length - 1) > MAX_WALK) throw new PageTooFar(`page ${target + 1} is too far`);
  while (cursors.length <= target) {
    const cursor = await next(cursors[cursors.length - 1]);
    if (cursor === null) break;
    cursors.push(cursor);
  }
  return { cursors, index: Math.min(target, cursors.length - 1) };
}
