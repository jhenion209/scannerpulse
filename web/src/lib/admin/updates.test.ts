import { describe, expect, it } from 'vitest';
import { canAsk, helperText, latestText, newestText, requestState, type HelperRequest, type UpdatesInfo } from './updates';

const info = (over: Partial<UpdatesInfo> = {}): UpdatesInfo => ({
  running: '0.9.0',
  machines: [],
  newest: null,
  check: true,
  helper: { host: 'radio-a', version: '0.9.0', seen_seconds_ago: 3, up: true },
  requests: { update: null, switchover: null },
  open: false,
  ...over,
});

const request = (over: Partial<HelperRequest> = {}): HelperRequest => ({
  id: 1,
  kind: 'update',
  requested_by: 'ann',
  requested_at: '2026-10-19T20:00:00Z',
  started_at: null,
  finished_at: null,
  host: null,
  ok: null,
  said: '',
  ...over,
});

describe('updates in words', () => {
  it('says whether an update is available', () => {
    expect(newestText(info())).toEqual({ text: expect.stringContaining('once a day'), available: false });
    expect(newestText(info({ check: false })).text).toContain('updates.check');
    const later = { version: '0.9.1', newer: true, notes: '', checked_at: '2026-10-19T20:00:00Z' };
    expect(newestText(info({ newest: later }))).toEqual({
      text: 'Update available: 0.9.1 (this site runs 0.9.0).',
      available: true,
    });
    expect(newestText(info({ newest: { ...later, version: '0.9.0', newer: false } }))).toEqual({
      text: 'Up to date: 0.9.0 is the newest release.',
      available: false,
    });
  });

  it('says whether the root helper runs, and how to start it', () => {
    expect(helperText(info())).toEqual({ text: 'The root helper runs on radio-a (0.9.0).', ok: true });
    expect(helperText(info({ helper: null })).text).toContain('agent install');
    const silent = helperText(info({ helper: { host: 'radio-a', version: '0.9.0', seen_seconds_ago: 90, up: false } }));
    expect(silent.ok).toBe(false);
    expect(silent.text).toContain('90 s');
  });

  it('lets an admin ask only when the helper is up and no request is open', () => {
    expect(canAsk(info())).toBe(true);
    expect(canAsk(info({ helper: null }))).toBe(false);
    expect(canAsk(info({ open: true, requests: { update: null, switchover: request({ kind: 'switchover' }) } }))).toBe(false);
  });

  it('follows a request from waiting to done or stopped', () => {
    expect(requestState(request()).text).toBe('The update waits for the root helper.');
    expect(requestState(request({ kind: 'switchover', started_at: 'x', host: 'radio-a' })).text).toBe(
      'The switchover runs on radio-a.',
    );
    expect(requestState(request({ started_at: 'x', finished_at: 'y', ok: true }))).toEqual({
      text: 'The update is done.',
      ok: true,
    });
    expect(requestState(request({ started_at: 'x', finished_at: 'y', ok: false })).ok).toBe(false);
  });

  it('names each step of the conversion the helper carries out', () => {
    expect(requestState(request({ kind: 'db_switch', started_at: 'x', host: 'radio-a' })).text).toBe(
      "The database's switch runs on radio-a.",
    );
    expect(requestState(request({ kind: 'archive_check', started_at: 'x', finished_at: 'y', ok: false }))).toEqual({
      text: 'The check of the copy stopped: see its last line.',
      ok: false,
    });
    expect(latestText('update')).toBe('The latest update');
    expect(latestText('db_retire')).toBe("The latest old database's removal");
  });
});
