import { describe, expect, it } from 'vitest';
import { LENGTH_FULL_MS, callTime, lengthPercent, seconds, source } from './format';

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

  it('shows only the time for today and the date before that', () => {
    const now = new Date(2026, 9, 15, 21, 0, 0);
    expect(callTime(new Date(2026, 9, 15, 14, 2, 11).toISOString(), now)).toBe('14:02:11');
    expect(callTime(new Date(2026, 9, 14, 23, 59, 1).toISOString(), now)).toBe('2026-10-14 23:59:01');
  });

  it('names the source by its alias, else its number', () => {
    expect(source({ radio_alias: 'Engine 7', radio_id: 4021 })).toBe('Engine 7');
    expect(source({ radio_alias: null, radio_id: 4021 })).toBe('4021');
    expect(source({ radio_alias: null, radio_id: null })).toBe('');
  });
});
