import { afterEach, describe, expect, it, vi } from 'vitest';
import { choices, startChoices } from './choices.svelte';

const filters = { talkgroup_groups: [], systems: [], sites: [] };

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('reading the filter choices', () => {
  it('reads again after 30 seconds when the first read failed', async () => {
    vi.useFakeTimers();
    let calls = 0;
    vi.stubGlobal('fetch', async (url: string) => {
      if (url.includes('/api/filters') && ++calls === 1) throw new Error('down');
      return new Response(JSON.stringify(url.includes('/api/filters') ? filters : []), { status: 200 });
    });
    const stop = startChoices();
    await vi.advanceTimersByTimeAsync(0);
    expect(choices.error).toContain('down');
    await vi.advanceTimersByTimeAsync(29_000);
    expect(choices.error).not.toBeNull();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(choices.error).toBeNull();
    stop();
  });

  it('does not retry after the stop', async () => {
    vi.useFakeTimers();
    const fetcher = vi.fn(async () => {
      throw new Error('down');
    });
    vi.stubGlobal('fetch', fetcher);
    const stop = startChoices();
    await vi.advanceTimersByTimeAsync(0);
    const first = fetcher.mock.calls.length;
    stop();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fetcher.mock.calls.length).toBe(first);
  });
});
