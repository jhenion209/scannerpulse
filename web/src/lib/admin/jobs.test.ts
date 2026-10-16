import { describe, expect, it } from 'vitest';
import { activeCount, canStop, kindName, percent, progressText, stateText, waitingFor, type Job } from './jobs';

const job = (over: Partial<Job> = {}): Job => ({
  id: 1, kind: 'queue', params: {}, state: 'running', done: 40, total: 100, message: '', started_by: 'ann',
  created_at: '2031-07-01T10:00:00Z', started_at: '2031-07-01T10:00:05Z', finished_at: null, updated_at: '2031-07-01T10:05:00Z',
  host: 'host-a', stop_requested: false, ...over,
});

describe('a job', () => {
  it('says what it is, how far it has come and how much is done', () => {
    expect(kindName('queue')).toBe('Queue past calls for transcription');
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
