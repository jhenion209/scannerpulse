import { describe, expect, it } from 'vitest';
import { about, clockSentence, clockWarnings, failoverChecks, locationWarnings, refereeWarning, type AdminStatus, leaseText, transcriptState } from './health';

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

describe('the referee warning', () => {
  it('warns only of a referee that does not answer', () => {
    expect(refereeWarning(null)).toBeNull();
    // A status read without the field (an older server, a fixture).
    expect(refereeWarning(undefined as unknown as AdminStatus['referee'])).toBeNull();
    expect(refereeWarning({ host: 'ref.example.org', answers: true })).toBeNull();
    expect(refereeWarning({ host: 'ref.example.org', answers: false, error: 'no answer within 3 s' })).toEqual({
      text: 'Referee unreachable: automatic failover is off until it returns',
      reason: 'no answer within 3 s',
    });
  });
});

describe('the storage warnings', () => {
  it('names each location a running host cannot use', () => {
    expect(locationWarnings(undefined)).toEqual([]);
    expect(locationWarnings([{ location: 'Cold 2024', host: 'host-b', problem: 'it is not mounted' }])).toEqual([
      'host-b cannot use the storage location Cold 2024: it is not mounted. Its calls there do not play from host-b.',
    ]);
  });
});

describe('the clocks', () => {
  it('names each machine or recorder whose clock needs a look', () => {
    expect(clockWarnings(undefined)).toEqual([]);
    expect(
      clockWarnings([
        { subject: 'db1', text: 'its time service is off' },
        { subject: 'south', text: "its clock is 2.0 s ahead of north's (6 copies): one call can be kept as two" },
      ]),
    ).toEqual([
      'db1: its time service is off.',
      "south: its clock is 2.0 s ahead of north's (6 copies): one call can be kept as two.",
    ]);
  });

  it("says a recorder's clock warning as a sentence", () => {
    expect(clockSentence('its files are seen 90.0 s after their calls end')).toBe(
      'Its files are seen 90.0 s after their calls end.',
    );
  });
});
