import { describe, expect, it } from 'vitest';
import type { Call } from './api';
import {
  COLUMNS,
  EXTRA_COLUMNS,
  defaultColumns,
  extraText,
  formatColumns,
  parseColumns,
  toggleColumn,
} from './columns';

const call = (more: Partial<Call> = {}): Call => ({
  id: 31015, started_at: '2026-10-15T12:00:00Z', duration_ms: 1000, talkgroup_id: 3, talkgroup: 101,
  talkgroup_name: 'Fire Dispatch', system: 'Metro', site: 'North', radio_id: 4021, radio_alias: null,
  transcript: null, origin: 'human', ...more,
});

describe('the default columns', () => {
  it('are today\'s six on a desk, in the table\'s order', () => {
    expect(defaultColumns(false)).toEqual(['time', 'talkgroup', 'site', 'source', 'length', 'text']);
  });

  it('are the phone\'s four, with no site or source', () => {
    expect(defaultColumns(true)).toEqual(['time', 'talkgroup', 'length', 'text']);
  });

  it('never include the four extras', () => {
    for (const phone of [false, true]) {
      const set = defaultColumns(phone);
      for (const id of ['system', 'number', 'id', 'origin'] as const) expect(set).not.toContain(id);
    }
  });
});

describe('the saved choice', () => {
  it('is nothing when nothing was stored', () => {
    expect(parseColumns(null)).toBeNull();
  });

  it('keeps the known columns once each, in the table\'s order', () => {
    expect(parseColumns('text,time,time,id')).toEqual(['time', 'id', 'text']);
  });

  it('ignores spaces around the names, as a hand-edited value may have them', () => {
    expect(parseColumns('time, text ,id')).toEqual(['time', 'id', 'text']);
  });

  it('drops unknown names, and a value with none of ours is as good as nothing', () => {
    expect(parseColumns('time,bogus')).toEqual(['time']);
    expect(parseColumns('bogus,<b>')).toBeNull();
  });

  it('may be an empty list: every column was hidden on purpose', () => {
    expect(parseColumns('')).toEqual([]);
  });

  it('is written as a comma list that reads back the same', () => {
    const chosen = ['time', 'system', 'text'] as const;
    expect(parseColumns(formatColumns([...chosen]))).toEqual([...chosen]);
    expect(formatColumns([])).toBe('');
  });
});

describe('ticking a column', () => {
  it('adds it in the table\'s order, not at the end', () => {
    expect(toggleColumn(['time', 'text'], 'site')).toEqual(['time', 'site', 'text']);
  });

  it('removes it when it is already there', () => {
    expect(toggleColumn(['time', 'site', 'text'], 'site')).toEqual(['time', 'text']);
  });
});

describe('the extra columns', () => {
  it('are the six a phone shows as labels under the text, each with its own name', () => {
    expect(EXTRA_COLUMNS).toEqual(['number', 'system', 'site', 'source', 'id', 'origin']);
    expect(COLUMNS.map((c) => c.id).sort()).toEqual(
      ['id', 'length', 'number', 'origin', 'site', 'source', 'system', 'talkgroup', 'text', 'time'],
    );
  });

  it('read their text from the call, and are empty when the call lacks the value', () => {
    expect(extraText(call(), 'system')).toBe('Metro');
    expect(extraText(call(), 'number')).toBe('101');
    expect(extraText(call(), 'id')).toBe('31015');
    expect(extraText(call(), 'origin')).toBe('human');
    expect(extraText(call(), 'site')).toBe('North');
    expect(extraText(call(), 'source')).toBe('4021');
    expect(extraText(call({ system: null, talkgroup: null, origin: null, site: null, radio_id: null }), 'system')).toBe('');
    expect(extraText(call({ talkgroup: null }), 'number')).toBe('');
    expect(extraText(call({ origin: null }), 'origin')).toBe('');
  });
});
