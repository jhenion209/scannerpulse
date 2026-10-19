import { describe, expect, it } from 'vitest';
import { bytesText, formerPrimaryText, machineText, modeText, standbyText } from './layout';

describe('the layout in words', () => {
  it('names the failover mode', () => {
    expect(modeText('one')).toBe('One machine: no failover.');
    expect(modeText('hand')).toContain('promoted by hand');
    expect(modeText('referee')).toContain('the referee promotes the standby by itself');
    expect(modeText('unknown')).toContain('join');
  });

  it('gives sizes in bytes, kB, MB and GB', () => {
    expect(bytesText(0)).toBe('0 bytes');
    expect(bytesText(900)).toBe('900 bytes');
    expect(bytesText(2048)).toBe('2.0 kB');
    expect(bytesText(5 * 1024 * 1024)).toBe('5.0 MB');
    expect(bytesText(3 * 1024 ** 3)).toBe('3.0 GB');
  });

  it('says how far a standby is behind', () => {
    expect(standbyText({ behind_bytes: 0, replayed_seconds_ago: 4 })).toEqual({
      text: 'in step (its last replayed write 4 s ago)',
      ok: true,
    });
    expect(standbyText({ behind_bytes: 2048, replayed_seconds_ago: null }).text).toBe('2.0 kB behind');
    // Far behind: worth a look.
    expect(standbyText({ behind_bytes: 64 * 1024 ** 2, replayed_seconds_ago: 30 }).ok).toBe(false);
    expect(standbyText({ behind_bytes: null, replayed_seconds_ago: null }).text).toContain('no primary answers');
  });

  it('says whether each machine answers', () => {
    expect(machineText({ up: true, version: '0.7.0', seen_seconds_ago: 3 })).toEqual({
      text: 'answering (version 0.7.0)',
      ok: true,
    });
    expect(machineText({ up: false, version: '0.7.0', seen_seconds_ago: 120 })).toEqual({
      text: 'not heard from for 2 min',
      ok: false,
    });
  });

  it('tells what to do with a former primary', () => {
    expect(formerPrimaryText).toBe(
      'answers as a primary on an older timeline: run sudo /usr/local/bin/scannerpulse db rejoin there',
    );
  });

  it('says a machine that never beat was never heard from', () => {
    expect(machineText({ up: false, version: null, seen_seconds_ago: null })).toEqual({
      text: 'never heard from (a version before 0.7.0, or its web has not started)',
      ok: false,
    });
  });
});
