import { describe, expect, it } from 'vitest';
import { searchAsk, searchOf, withSearch } from './search';

describe('the search panel in the address', () => {
  it('reads the words a link searches for', () => {
    expect(searchOf('?all=1&search=engine%207')).toBe('engine 7');
    expect(searchOf('?search=')).toBeNull();
    expect(searchOf('?all=1')).toBeNull();
  });

  it('adds the words to the list\'s own query, or takes them away', () => {
    expect(withSearch('?all=1', 'engine 7')).toBe('?all=1&search=engine+7');
    expect(withSearch('', 'fire')).toBe('?search=fire');
    expect(withSearch('?all=1&search=old', null)).toBe('?all=1');
    expect(withSearch('?search=old', '  ')).toBe('');
  });
});

describe('what the panel asks the server', () => {
  it('searches every date when no date is given, with an exact count', () => {
    const ask = searchAsk({ text: 'engine', talkgroup: null, system: null, from: '', to: '', size: 25 }, 'UTC');
    expect(ask.filters.allDates).toBe(true);
    expect(ask.filters.talkgroups).toEqual([]);
    expect(ask.text).toBe('engine');
    expect(ask.exact).toBe(true);
    expect(ask.size).toBe(25);
  });

  it('keeps a range when one is given, and a talkgroup or system when chosen', () => {
    const ask = searchAsk({ text: '', talkgroup: 4, system: 2, from: '2031-07-01T08:00', to: '', size: 25 }, 'Asia/Tokyo');
    expect(ask.filters).toMatchObject({ talkgroups: [4], system: 2, from: '2031-07-01T08:00', to: null, allDates: false });
    expect(ask.zone).toBe('Asia/Tokyo');
  });
});
