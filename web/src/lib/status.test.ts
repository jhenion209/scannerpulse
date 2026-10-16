import { describe, expect, it } from 'vitest';
import { STALE_AFTER_MS, ageOf, callsText, clockText, durationText, etaText, isStale, newer } from './status';

describe('the status chips', () => {
  it('say how old the newest call is, by the server\'s clock', () => {
    expect(ageOf('2031-03-01T12:00:00Z', new Date('2031-03-01T12:04:30Z'))).toBe(270_000);
    expect(ageOf(null, new Date())).toBeNull();
    // A clock a little behind the newest call is not a negative age.
    expect(ageOf('2031-03-01T12:00:05Z', new Date('2031-03-01T12:00:00Z'))).toBe(0);
  });

  it('warn when the newest call is older than 15 minutes, or when there is none', () => {
    expect(STALE_AFTER_MS).toBe(15 * 60_000);
    expect(isStale(STALE_AFTER_MS)).toBe(false);
    expect(isStale(STALE_AFTER_MS + 1)).toBe(true);
    expect(isStale(null)).toBe(true);
  });

  it('write a duration in the largest unit or two that matter', () => {
    expect(durationText(12_000)).toBe('12 s');
    expect(durationText(4 * 60_000 + 59_000)).toBe('4 min');
    expect(durationText(2 * 3_600_000 + 5 * 60_000)).toBe('2 h 5 min');
    expect(durationText(3 * 86_400_000 + 7 * 3_600_000)).toBe('3 d 7 h');
  });

  it('give the queue\'s end as an estimate, and nothing when there is none', () => {
    expect(etaText(900)).toBe('about 15 min');
    expect(etaText(30)).toBe('about 1 min');
    expect(etaText(null)).toBe('');
  });

  it('count the calls, and say "about" for an estimate', () => {
    expect(callsText(35, false)).toBe('35 calls');
    expect(callsText(1, false)).toBe('1 call');
    expect(callsText(16_137_111, true)).toBe('about 16,137,111 calls');
  });

  it('show the time in the site\'s zone, with its short name', () => {
    expect(clockText(new Date('2031-03-01T18:02:11Z'), 'America/Chicago')).toBe('12:02:11 CST');
    expect(clockText(new Date('2031-03-01T18:02:11Z'), 'UTC')).toBe('18:02:11 UTC');
    // A zone the browser does not know falls back to UTC rather than failing.
    expect(clockText(new Date('2031-03-01T18:02:11Z'), 'Nowhere/Else')).toBe('18:02:11 UTC');
  });

  it('keep the newest call time, whichever of the two is later', () => {
    expect(newer('2031-03-01T12:00:00Z', '2031-03-01T12:00:05Z')).toBe('2031-03-01T12:00:05Z');
    expect(newer('2031-03-01T12:00:05Z', '2031-03-01T12:00:00Z')).toBe('2031-03-01T12:00:05Z');
    expect(newer(null, '2031-03-01T12:00:00Z')).toBe('2031-03-01T12:00:00Z');
  });
});
