import { describe, expect, it } from 'vitest';
import { hourly } from './reports';

describe('hourly', () => {
  it('sums the twelve five-minute slots of each hour', () => {
    const slots = Array.from({ length: 288 }, () => 0.5);
    slots[13] = 2; // 01:05
    const hours = hourly(slots);
    expect(hours).toHaveLength(24);
    expect(hours[0]).toBe(6);
    expect(hours[1]).toBe(7.5);
    expect(hours[23]).toBe(6);
  });
});
