import { describe, expect, it } from 'vitest';
import { about, failoverChecks, leaseText, transcriptState } from './health';

describe('the lease', () => {
  it('says who holds it and for how long', () => {
    expect(leaseText({ valid: true, holder: 'host-a', expires_in: 25.4, heartbeats: [] })).toBe(
      'host-a holds the lease, for 25 s more unless renewed.',
    );
  });

  it('says when it ran out, was let go, or was never taken', () => {
    expect(leaseText({ valid: false, holder: 'host-a', expires_in: -125, heartbeats: [] })).toBe(
      'host-a held the lease; it ran out 2 min ago.',
    );
    expect(leaseText({ valid: false, holder: '', expires_in: 0, heartbeats: [] })).toBe(
      'No host holds the lease: the last holder let it go.',
    );
    expect(leaseText({ valid: false, holder: null, expires_in: 0, heartbeats: [] })).toBe(
      'Ingest has never run against this database.',
    );
  });
});

describe('the checks after a failover', () => {
  it('are fine when the lease is held and nothing is left over', () => {
    const checks = failoverChecks({
      lease_held: true, jobs_running: 1, jobs_stuck: 0, calls_failed: 0, calls_filed_twice: 0, files_left_in_sources: 0,
    });
    expect(checks.every((c) => c.ok)).toBe(true);
    expect(checks[0].value).toBe('yes');
  });

  it('each need a look when something is left over', () => {
    const checks = failoverChecks({
      lease_held: false, jobs_running: 0, jobs_stuck: 2, calls_failed: 1234, calls_filed_twice: 1, files_left_in_sources: 3,
    });
    expect(checks.map((c) => c.ok)).toEqual([false, false, false, false, false]);
    expect(checks[2].value).toBe('1,234');
  });
});

describe('words for the figures', () => {
  it('names a call state and an estimate', () => {
    expect(transcriptState('untried')).toBe('not tried');
    expect(transcriptState('no_speech')).toBe('no speech');
    expect(transcriptState('something new')).toBe('something new');
    expect(about(16000000)).toBe('about 16,000,000');
    expect(about(null)).toBe('—');
  });
});
