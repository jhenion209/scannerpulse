import { describe, expect, it } from 'vitest';
import { drainedPercent } from './drain';

describe('the queue draining', () => {
  it('is the whole per cent gone since the largest the queue was seen at', () => {
    expect(drainedPercent(200, 50)).toBe(75);
    expect(drainedPercent(200, 200)).toBe(0);
    expect(drainedPercent(200, 0)).toBe(100);
  });

  it('rounds down exactly: 100 only once no call waits, and no float error costs a point', () => {
    expect(drainedPercent(100, 1)).toBe(99);
    expect(drainedPercent(100, 0)).toBe(100);
    expect(drainedPercent(100, 71)).toBe(29);
    expect(drainedPercent(100, 57)).toBe(43);
    expect(drainedPercent(100, 44)).toBe(56);
    expect(drainedPercent(100, 42)).toBe(58);
    for (let queue = 0; queue <= 100; queue++) expect(drainedPercent(100, queue)).toBe(100 - queue);
  });

  it('is unknown while the queue never held a call, and never past either end', () => {
    expect(drainedPercent(0, 0)).toBeNull();
    expect(drainedPercent(10, 12)).toBe(0);
  });
});
