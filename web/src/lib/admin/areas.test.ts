import { describe, expect, it } from 'vitest';
import { AREAS, areaOf } from './areas';

describe('the area a link names', () => {
  it('is the one after the #, or the first when it names none', () => {
    for (const area of AREAS) expect(areaOf(`#${area.id}`)).toBe(area.id);
    expect(areaOf('')).toBe(AREAS[0].id);
    expect(areaOf('#nowhere')).toBe(AREAS[0].id);
  });
});
