import { describe, expect, it } from 'vitest';
import { LENGTH_FULL_MS, callTime, lengthPercent, seconds, source, zonedInstant } from './format';

describe('a row', () => {
  it('fills the length bar up to 8 seconds, then stays full', () => {
    expect(LENGTH_FULL_MS).toBe(8000);
    expect(lengthPercent(2000)).toBe(25);
    expect(lengthPercent(30_000)).toBe(100);
    expect(lengthPercent(null)).toBe(0);
  });

  it('shows the length in seconds with one decimal', () => {
    expect(seconds(2449)).toBe('2.4 s');
    expect(seconds(null)).toBe('');
  });

  it("shows the time in the site's zone, with the date when it is not that day there", () => {
    const now = new Date('2031-07-01T20:00:00Z'); // 05:00 on the 2nd in Tokyo
    expect(callTime('2031-07-01T16:30:05Z', now, 'Asia/Tokyo')).toBe('01:30:05');
    expect(callTime('2031-07-01T14:59:59Z', now, 'Asia/Tokyo')).toBe('2031-07-01 23:59:59');
    expect(callTime('2031-07-01T16:30:05Z', now, 'UTC')).toBe('16:30:05');
    expect(callTime('2031-06-30T23:59:59Z', now, 'UTC')).toBe('2031-06-30 23:59:59');
  });

  it('shows UTC for a zone the browser does not know', () => {
    expect(callTime('2031-07-01T16:30:05Z', new Date(0), 'Nowhere/Special')).toBe('2031-07-01 16:30:05');
  });

  it("reads a date picker's time in the site's zone", () => {
    expect(zonedInstant('2031-07-01T08:00', 'Asia/Tokyo')?.toISOString()).toBe('2031-06-30T23:00:00.000Z');
    expect(zonedInstant('2031-07-01T08:00', 'America/New_York')?.toISOString()).toBe('2031-07-01T12:00:00.000Z');
    expect(zonedInstant('2031-01-15T08:00', 'America/New_York')?.toISOString()).toBe('2031-01-15T13:00:00.000Z');
    expect(zonedInstant('2031-07-01T08:00', 'UTC')?.toISOString()).toBe('2031-07-01T08:00:00.000Z');
  });

  it('reads a time the clock skips as the hour after, and a repeated one as the first', () => {
    // Clocks go forward at 02:00 on 9 March 2031 in New York and at 02:00 on
    // 30 March 2031 in Berlin; back at 02:00 on 2 November and 03:00 on 26 October.
    expect(zonedInstant('2031-03-09T02:30', 'America/New_York')?.toISOString()).toBe('2031-03-09T07:30:00.000Z');
    expect(zonedInstant('2031-03-30T02:30', 'Europe/Berlin')?.toISOString()).toBe('2031-03-30T01:30:00.000Z');
    expect(zonedInstant('2031-11-02T01:30', 'America/New_York')?.toISOString()).toBe('2031-11-02T05:30:00.000Z');
    expect(zonedInstant('2031-10-26T02:30', 'Europe/Berlin')?.toISOString()).toBe('2031-10-26T00:30:00.000Z');
  });

  it('names no instant for a day or a time that does not exist', () => {
    expect(zonedInstant('2031-02-30T08:00', 'UTC')).toBeNull();
    expect(zonedInstant('2031-02-28T25:00', 'UTC')).toBeNull();
    expect(zonedInstant('2031-02-28', 'UTC')).toBeNull();
  });

  it('names the source by its alias, else its number', () => {
    expect(source({ radio_alias: 'Engine 7', radio_id: 4021 })).toBe('Engine 7');
    expect(source({ radio_alias: null, radio_id: 4021 })).toBe('4021');
    expect(source({ radio_alias: null, radio_id: null })).toBe('');
  });
});
