import { describe, expect, it } from 'vitest';
import { formatMine, MAX_KEPT, nextStars, parseMine } from './ratings';

describe('the stars kept in this browser', () => {
  it('reads what was stored', () => {
    expect(parseMine('{"12":4,"7":1}')).toEqual({ 12: 4, 7: 1 });
  });

  it('drops anything malformed', () => {
    expect(parseMine(null)).toEqual({});
    expect(parseMine('not json')).toEqual({});
    expect(parseMine('[1,2]')).toEqual({});
    expect(parseMine('{"1":0,"2":6,"3":2.5,"x":3,"-4":3,"5":"4","9":5}')).toEqual({ 9: 5 });
  });

  it('keeps the newest calls past the limit', () => {
    const many: Record<number, number> = {};
    for (let id = 1; id <= MAX_KEPT + 5; id += 1) many[id] = 3;
    const kept = parseMine(formatMine(many));
    expect(Object.keys(kept)).toHaveLength(MAX_KEPT);
    expect(kept[MAX_KEPT + 5]).toBe(3);
    expect(kept[5]).toBeUndefined();
    expect(kept[6]).toBe(3);
  });
});

describe('a tap on a star', () => {
  it('sets it, changes it, and clears it by tapping the star already given', () => {
    expect(nextStars(undefined, 4)).toBe(4);
    expect(nextStars(2, 5)).toBe(5);
    expect(nextStars(3, 3)).toBeNull();
  });
});
