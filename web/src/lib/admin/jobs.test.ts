import { describe, expect, it } from 'vitest';
import { activeCount, canStop, kindName, percent, progressText, runningNow, stateText, waitingFor, type Job } from './jobs';
import type { HelperRequest } from './updates';

const job = (over: Partial<Job> = {}): Job => ({
  id: 1, kind: 'queue', params: {}, state: 'running', done: 40, total: 100, message: '', started_by: 'ann',
  created_at: '2031-07-01T10:00:00Z', started_at: '2031-07-01T10:00:05Z', finished_at: null, updated_at: '2031-07-01T10:05:00Z',
  host: 'host-a', stop_requested: false, ...over,
});

describe('a job', () => {
  it('says what it is, how far it has come and how much is done', () => {
    expect(kindName('queue')).toBe('Queue past calls for transcription');
    expect(kindName('move')).toBe('Move a date range');
    expect(kindName('purge_before')).toBe('Purge older than a day');
    expect(kindName('import_review')).toBe('Review an older archive');
    expect(kindName('import')).toBe('Import an older archive');
    expect(kindName('consolidate')).toBe('Consolidate an import');
    expect(kindName('something new')).toBe('something new');
    expect(progressText(job({ done: 1234, total: 5000 }))).toBe('1,234 of 5,000');
    expect(progressText(job({ total: null }))).toBe('40');
    expect(percent(job())).toBe(40);
    expect(percent(job({ total: null }))).toBeNull();
    expect(percent(job({ total: 0, done: 0 }))).toBeNull();
  });

  it('gives the reason a step waits', () => {
    expect(waitingFor(job({ message: 'waiting: maintenance is on' }))).toBe('maintenance is on');
    expect(waitingFor(job({ message: 'queued 40 calls' }))).toBeNull();
  });

  it('can be stopped once, until it ends', () => {
    expect(canStop(job())).toBe(true);
    expect(canStop(job({ state: 'queued' }))).toBe(true);
    expect(canStop(job({ stop_requested: true }))).toBe(false);
    expect(canStop(job({ state: 'done' }))).toBe(false);
    expect(stateText(job({ stop_requested: true }))).toBe('stopping after its current step');
    expect(stateText(job({ state: 'failed' }))).toBe('failed');
  });
});

describe('the badge', () => {
  it('counts the jobs that have not ended', () => {
    expect(activeCount([job(), job({ state: 'queued' }), job({ state: 'done' }), job({ state: 'stopped' })])).toBe(2);
    expect(activeCount([])).toBe(0);
  });
});

describe('what runs now', () => {
  const request = (over: Partial<HelperRequest>): HelperRequest => ({
    id: 5, kind: 'archive_switch', requested_by: 'ann', requested_at: '2031-07-01T10:00:00Z', started_at: null,
    finished_at: null, host: null, ok: null, said: '', ...over,
  });

  it('lists the running jobs, then those waiting in their line, then the helper requests not ended', () => {
    const rows = runningNow(
      [
        job({ id: 9, state: 'queued', kind: 'purge_before', started_by: 'root on sp-a' }),
        job({ id: 7, state: 'done' }),
        job({ id: 8, state: 'running', kind: 'move', done: 2, total: 5, started_by: 'cy', started_at: '2031-07-01T09:00:00Z' }),
        job({ id: 6, state: 'queued', kind: 'queue', started_by: 'ann' }),
      ],
      [request({}), request({ id: 4, kind: 'update', finished_at: '2031-07-01T09:30:00Z' }), null, undefined],
    );
    expect(rows.map((r) => [r.label, r.what, r.progress, r.by, r.href])).toEqual([
      ['Job 8', 'Move a date range', '2 of 5', 'cy', '#jobs'],
      ['Job 6', 'Queue past calls for transcription', 'waiting', 'ann', '#jobs'],
      ['Job 9', 'Purge older than a day', 'waiting', 'root on sp-a', '#jobs'],
      ['Request 5', "The archive's switch", 'waiting', 'ann', '#layout'],
    ]);
    expect(rows[0].at).toBe('2031-07-01T09:00:00Z');
  });

  it('names an update in progress, which the Updates area shows', () => {
    const [row] = runningNow([], [request({ kind: 'update', started_at: '2031-07-01T10:01:00Z' })]);
    expect([row.progress, row.href, row.at]).toEqual(['running', '#updates', '2031-07-01T10:01:00Z']);
  });
});
