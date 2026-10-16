import { describe, expect, it } from 'vitest';
import {
  NO_FILTERS,
  favoriteUnresolved,
  filtersToSearch,
  listQuery,
  parseFilters,
  talkgroupsAskedFor,
} from './filters';

const favorites = [{ id: 3, talkgroups: [12, 7] }];
const now = new Date('2031-03-01T12:00:00Z');

describe('filters in the URL', () => {
  it('reads back what it wrote', () => {
    const filters = {
      ...NO_FILTERS,
      favorite: 3,
      talkgroups: [7, 9],
      system: 2,
      from: '2031-02-28T08:00',
      to: '2031-02-28T20:00',
      smart: true,
    };
    expect(parseFilters(filtersToSearch(filters))).toEqual(filters);
  });

  it('leaves the defaults out of the URL', () => {
    expect(filtersToSearch(NO_FILTERS)).toBe('');
  });

  it('drops what is malformed', () => {
    const filters = parseFilters('?talkgroups=7,x,-1,7,99999999999999999999&system=a&from=yesterday&favorite=2.5');
    expect(filters).toEqual({ ...NO_FILTERS, talkgroups: [7] });
  });

  it('all dates wins over a range', () => {
    const filters = { ...NO_FILTERS, allDates: true, from: '2031-02-28T08:00' };
    expect(filtersToSearch(filters)).toBe('?all=1');
  });

  it('drops a date with the right shape but no such day or time', () => {
    expect(parseFilters('?from=2031-13-45T99:99').from).toBeNull();
    expect(parseFilters('?to=2031-02-28T25:00').to).toBeNull();
  });
});

describe('the request', () => {
  it('asks for the rolling last 24 hours by default', () => {
    const query = listQuery(NO_FILTERS, favorites, { size: 100 }, now);
    expect(query.get('from')).toBe('2031-02-28T12:00:00.000Z');
    expect(query.has('until')).toBe(false);
    expect(query.get('size')).toBe('100');
  });

  it('asks for every date when all dates is on', () => {
    const query = listQuery({ ...NO_FILTERS, allDates: true }, favorites, { size: 50 }, now);
    expect(query.has('from')).toBe(false);
  });

  it('joins the favorite talkgroups and the chosen ones, once each', () => {
    expect(talkgroupsAskedFor({ ...NO_FILTERS, favorite: 3, talkgroups: [7, 30] }, favorites)).toEqual([
      7, 12, 30,
    ]);
  });

  it('carries the cursor and skips the count on a page turn', () => {
    const query = listQuery(NO_FILTERS, favorites, { size: 25, after: 'abc', count: false }, now);
    expect(query.get('after')).toBe('abc');
    expect(query.get('count')).toBe('false');
  });

  it('asks the server for SmartSort; the page never sorts', () => {
    const query = listQuery({ ...NO_FILTERS, smart: true }, favorites, { size: 25 }, now);
    expect(query.get('smart')).toBe('true');
  });

  it('falls back to the rolling window when no date given to it is real', () => {
    const query = listQuery({ ...NO_FILTERS, from: '2031-13-45T99:99' }, favorites, { size: 25 }, now);
    expect(query.get('from')).toBe('2031-02-28T12:00:00.000Z');
    expect(query.has('until')).toBe(false);
  });

  it('keeps the real bound when only one of the dates is real', () => {
    const query = listQuery({ ...NO_FILTERS, from: '2031-02-28T08:00', to: '2031-13-45T99:99' }, favorites, { size: 25 }, now);
    expect(query.has('from')).toBe(true);
    expect(query.has('until')).toBe(false);
  });
});

describe('a favorite that gives no talkgroups', () => {
  it('is unresolved for an unknown favorite, and for one with no talkgroups', () => {
    expect(favoriteUnresolved({ ...NO_FILTERS, favorite: 99 }, favorites)).toBe(true);
    expect(favoriteUnresolved({ ...NO_FILTERS, favorite: 4 }, [{ id: 4, talkgroups: [] }])).toBe(true);
  });

  it('is not unresolved when it resolves, or when no favorite is chosen', () => {
    expect(favoriteUnresolved({ ...NO_FILTERS, favorite: 3 }, favorites)).toBe(false);
    expect(favoriteUnresolved(NO_FILTERS, favorites)).toBe(false);
  });

  it('asks for no talkgroup rather than every one', () => {
    const query = listQuery({ ...NO_FILTERS, favorite: 99 }, favorites, { size: 25 }, now);
    expect(query.get('talkgroups')).toBe('0');
  });
});
