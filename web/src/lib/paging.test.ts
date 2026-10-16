import { describe, expect, it } from 'vitest';
import { FIRST_PAGE, MAX_WALK, PageTooFar, pageCount, walkTo } from './paging';

describe('the page count', () => {
  it('rounds up and says when the total is capped', () => {
    expect(pageCount(0, false, 100)).toEqual({ pages: 1, more: false });
    expect(pageCount(101, false, 100)).toEqual({ pages: 2, more: false });
    expect(pageCount(10_000, true, 100)).toEqual({ pages: 100, more: true });
  });
});

describe('walking the cursors to a page', () => {
  // Page i's cursor is `c<i>`; `last` is the final page's index.
  const server = (last: number) => {
    const asked: (string | null)[] = [];
    const next = async (after: string | null) => {
      asked.push(after);
      const i = after === null ? 0 : Number(after.slice(1));
      return i < last ? `c${i + 1}` : null;
    };
    return { asked, next };
  };

  it('goes back to a known page without asking', async () => {
    const { asked, next } = server(9);
    const paging = { cursors: [null, 'c1', 'c2'], index: 2 };
    expect(await walkTo(paging, 1, next)).toEqual({ cursors: [null, 'c1', 'c2'], index: 1 });
    expect(asked).toEqual([]);
  });

  it('reads forward from the last known cursor', async () => {
    const { asked, next } = server(9);
    expect(await walkTo({ cursors: [null, 'c1'], index: 0 }, 3, next)).toEqual({
      cursors: [null, 'c1', 'c2', 'c3'],
      index: 3,
    });
    expect(asked).toEqual(['c1', 'c2']);
  });

  it('stops on the last page when asked for one past the end', async () => {
    const { next } = server(2);
    expect(await walkTo(FIRST_PAGE, 7, next)).toEqual({ cursors: [null, 'c1', 'c2'], index: 2 });
  });

  it('refuses a page more than the limit away, before asking anything', async () => {
    const { asked, next } = server(1000);
    await expect(walkTo(FIRST_PAGE, MAX_WALK + 1, next)).rejects.toBeInstanceOf(PageTooFar);
    expect(asked).toEqual([]);
  });
});
