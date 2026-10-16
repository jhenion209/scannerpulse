import { afterEach, describe, expect, it, vi } from 'vitest';
import { follow } from './feed';

class FakeSocket {
  static made: FakeSocket[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((m: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  closed = false;
  constructor(readonly url: string) {
    FakeSocket.made.push(this);
  }
  close() {
    this.closed = true;
  }
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  FakeSocket.made = [];
});

describe('the live feed', () => {
  it('passes each event on, and a message it cannot read as a resync', () => {
    vi.stubGlobal('WebSocket', FakeSocket);
    const events: unknown[] = [];
    follow(() => 'ws://x/api/live', (e) => events.push(e));
    FakeSocket.made[0].onmessage?.({ data: '{"type":"resync"}' });
    FakeSocket.made[0].onmessage?.({ data: 'not json' });
    expect(events).toEqual([{ type: 'resync' }, { type: 'resync' }]);
  });

  it('does not turn a fault in the handler into a resync', () => {
    vi.stubGlobal('WebSocket', FakeSocket);
    const seen: unknown[] = [];
    follow(() => 'ws://x/api/live', (e) => {
      seen.push(e);
      throw new Error('handler');
    });
    expect(() => FakeSocket.made[0].onmessage?.({ data: '{"type":"resync"}' })).toThrow('handler');
    expect(seen).toHaveLength(1);
  });

  it('opens again after a drop, later each time, asking the address again', () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', FakeSocket);
    let since = 1;
    follow(() => `ws://x/api/live?since=${since}`, () => {});
    since = 9;
    FakeSocket.made[0].onclose?.();
    vi.advanceTimersByTime(999);
    expect(FakeSocket.made).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(FakeSocket.made[1].url).toBe('ws://x/api/live?since=9');
    FakeSocket.made[1].onclose?.();
    vi.advanceTimersByTime(1999);
    expect(FakeSocket.made).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(FakeSocket.made).toHaveLength(3);
  });

  it('does not open again once stopped', () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', FakeSocket);
    const stop = follow(() => 'ws://x/api/live', () => {});
    stop();
    expect(FakeSocket.made[0].closed).toBe(true);
    FakeSocket.made[0].onclose?.();
    vi.advanceTimersByTime(60_000);
    expect(FakeSocket.made).toHaveLength(1);
  });
});
