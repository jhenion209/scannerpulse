import { describe, expect, it } from 'vitest';
import { pageState, toggleAll, toggleOne, keepOnPage } from './selection';

const page = [5, 4, 3, 2, 1];

describe('choosing calls', () => {
  it('starts with nothing chosen: a choice is a list of the calls ticked', () => {
    expect(pageState([], page)).toBe('none');
  });

  it('ticks and unticks one call', () => {
    expect(toggleOne([5], 3)).toEqual([5, 3]);
    expect(toggleOne([5, 3], 5)).toEqual([3]);
  });

  it('says whether none, some or all of the page is chosen', () => {
    expect(pageState([4], page)).toBe('some');
    expect(pageState(page, page)).toBe('all');
    expect(pageState([], [])).toBe('none');
  });

  it('ticks the whole page, or clears it when it is all ticked', () => {
    expect(toggleAll([4], page)).toEqual(page);
    expect(toggleAll(page, page)).toEqual([]);
  });

  it('keeps only the calls still on the page: another page starts with nothing chosen', () => {
    expect(keepOnPage([5, 9, 3], page)).toEqual([5, 3]);
    expect(keepOnPage([9, 8], page)).toEqual([]);
  });
});
