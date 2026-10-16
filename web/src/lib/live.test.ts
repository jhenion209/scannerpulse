import { describe, expect, it } from 'vitest';
import type { Call } from './api';
import { NO_FILTERS } from './filters';
import { addCall, liveApplies, liveUrl, newestId, setTranscript, wanted } from './live';

const call = (id: number, more: Partial<Call> = {}): Call => ({
  id, started_at: '2026-10-15T12:00:00Z', duration_ms: 1000, talkgroup_id: 1, talkgroup: 101,
  talkgroup_name: 'Fire Dispatch', system: 'SYSA', site: null, radio_id: null, radio_alias: null,
  transcript: null, origin: null, ...more,
});
const now = new Date('2026-10-15T12:00:00Z');

describe('when new calls join the list', () => {
  it('only on the first page, unpaused, with no range that has ended', () => {
    expect(liveApplies(NO_FILTERS, 0, false, now)).toBe(true);
    expect(liveApplies(NO_FILTERS, 1, false, now)).toBe(false);
    expect(liveApplies(NO_FILTERS, 0, true, now)).toBe(false);
    expect(liveApplies({ ...NO_FILTERS, allDates: true }, 0, false, now)).toBe(false);
    expect(liveApplies({ ...NO_FILTERS, to: '2026-10-14T10:00' }, 0, false, now)).toBe(false);
    expect(liveApplies({ ...NO_FILTERS, to: '2026-10-16T10:00' }, 0, false, now)).toBe(true);
  });
});

describe('which calls the list takes', () => {
  it('keeps the talkgroups asked for and the system chosen; the server sends every talkgroup when none is asked', () => {
    expect(wanted(call(1), [], null)).toBe(true);
    expect(wanted(call(1), [1, 2], null)).toBe(true);
    expect(wanted(call(1, { talkgroup_id: 3 }), [1, 2], null)).toBe(false);
    expect(wanted(call(1), [], 'SYSA')).toBe(true);
    expect(wanted(call(1), [], 'SYSB')).toBe(false);
  });
});

describe('a new call', () => {
  it('goes on top and pushes the others down without dropping one: the next page starts after the last row read', () => {
    expect(addCall([call(2), call(1)], call(3), 2)?.map((c) => c.id)).toEqual([3, 2, 1]);
  });

  it('is not added twice', () => {
    expect(addCall([call(2), call(1)], call(2), 2)).toBeNull();
  });

  it('older than every row of a full page belongs to a later page', () => {
    const old = call(9, { started_at: '2026-10-15T11:00:00Z' });
    expect(addCall([call(2), call(1)], old, 2)).toBeNull();
    expect(addCall([call(2)], old, 2)?.map((c) => c.id)).toEqual([2, 9]);
  });

  it('that ingest filed late goes in its place by time, not on top', () => {
    const late = call(9, { started_at: '2026-10-15T11:59:30Z' });
    const rows = [call(3, { started_at: '2026-10-15T12:00:00Z' }), call(2, { started_at: '2026-10-15T11:59:00Z' })];
    expect(addCall(rows, late, 3)?.map((c) => c.id)).toEqual([3, 9, 2]);
  });
});

describe('a transcript', () => {
  it('is written into its row in place', () => {
    const rows = setTranscript([call(2), call(1)], { type: 'transcript', id: 1, talkgroup_id: 1, transcript: 'engine 7', origin: 'human' });
    expect(rows?.[1]).toMatchObject({ id: 1, transcript: 'engine 7', origin: 'human' });
  });

  it('for a call not on the page changes nothing', () => {
    expect(setTranscript([call(2)], { type: 'transcript', id: 5, talkgroup_id: 1, transcript: 'x', origin: 'machine' })).toBeNull();
  });
});

describe('the feed address', () => {
  it('asks for the talkgroups and the calls since the newest seen, over wss on https', () => {
    expect(liveUrl({ protocol: 'https:', host: 'example.org' }, [3, 1], 42)).toBe(
      'wss://example.org/api/live?talkgroups=3%2C1&since=42',
    );
    expect(liveUrl({ protocol: 'http:', host: 'localhost:5173' }, [], null)).toBe('ws://localhost:5173/api/live');
  });

  it('remembers the newest call id seen', () => {
    expect(newestId([call(4), call(9), call(2)], 5)).toBe(9);
    expect(newestId([], null)).toBeNull();
  });
});
