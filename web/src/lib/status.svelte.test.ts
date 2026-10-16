import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Status } from './api';
import { fetchStatus } from './api';
import { noteCall, startStatus, status } from './status.svelte';
import { STATUS_REFRESH_MS } from './status';

vi.mock('./api', () => ({ fetchStatus: vi.fn() }));
const read = vi.mocked(fetchStatus);

const good = (over: Partial<Status> = {}): Status => ({
  maintenance: false,
  transcription: false,
  queue: 0,
  eta_seconds: null,
  calls: 35,
  calls_estimated: false,
  newest_call: '2031-03-01T12:00:00Z',
  server_time: '2031-03-01T12:00:30Z',
  time_zone: 'UTC',
  ...over,
});

beforeEach(() => {
  vi.useFakeTimers();
  read.mockReset();
  status.data = null;
  status.error = null;
  status.offset = 0;
});

afterEach(() => {
  vi.useRealTimers();
});

describe('reading the status', () => {
  it('shows an error and no data when the first read fails', async () => {
    read.mockRejectedValue(new Error('too slow'));
    const stop = startStatus();
    await vi.advanceTimersByTimeAsync(0);
    expect(status.data).toBeNull();
    expect(status.error).toBe('too slow');
    stop();
  });

  it('keeps the last good figures when a later read fails, and clears the error on the next good read', async () => {
    read.mockResolvedValueOnce(good());
    const stop = startStatus();
    await vi.advanceTimersByTimeAsync(0);
    expect(status.data?.calls).toBe(35);
    expect(status.error).toBeNull();

    read.mockRejectedValueOnce(new Error('too slow'));
    await vi.advanceTimersByTimeAsync(STATUS_REFRESH_MS);
    expect(status.data?.calls).toBe(35);
    expect(status.error).toBe('too slow');

    read.mockResolvedValueOnce(good({ calls: 36 }));
    await vi.advanceTimersByTimeAsync(STATUS_REFRESH_MS);
    expect(status.data?.calls).toBe(36);
    expect(status.error).toBeNull();
    stop();
  });

  it('sets the offset from the server\'s clock, and keeps a live call that is newer than the read', async () => {
    vi.setSystemTime(new Date('2031-03-01T12:00:00Z'));
    read.mockResolvedValueOnce(good());
    const stop = startStatus();
    await vi.advanceTimersByTimeAsync(0);
    expect(status.offset).toBe(30_000);

    noteCall('2031-03-01T12:00:20Z');
    read.mockResolvedValueOnce(good({ newest_call: '2031-03-01T12:00:10Z' }));
    await vi.advanceTimersByTimeAsync(STATUS_REFRESH_MS);
    expect(status.data?.newest_call).toBe('2031-03-01T12:00:20Z');
    stop();
  });

  it('stops reading after the stop', async () => {
    read.mockResolvedValue(good());
    const stop = startStatus();
    await vi.advanceTimersByTimeAsync(0);
    stop();
    const reads = read.mock.calls.length;
    await vi.advanceTimersByTimeAsync(STATUS_REFRESH_MS * 3);
    expect(read.mock.calls.length).toBe(reads);
  });
});
