import { describe, expect, it } from 'vitest';
import { drained } from './drain';

describe('the queue draining', () => {
  it('is the share gone since the largest the queue was seen at', () => {
    expect(drained(200, 50)).toBe(0.75);
    expect(drained(200, 200)).toBe(0);
    expect(drained(200, 0)).toBe(1);
  });

  it('is unknown while the queue never held a call, and never past either end', () => {
    expect(drained(0, 0)).toBeNull();
    expect(drained(10, 12)).toBe(0);
  });
});
