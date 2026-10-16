import { describe, expect, it } from 'vitest';
import { afterEnd, afterFailure, navKey, step } from './playback';

// Rows are newest first: 5 is the top row.
const calls = [5, 4, 3, 2, 1].map((id) => ({ id }));

describe('stepping through the list', () => {
  it('goes to the row above for newer and below for older', () => {
    expect(step(calls, 3, 'newer')?.id).toBe(4);
    expect(step(calls, 3, 'older')?.id).toBe(2);
  });

  it('stops at the page edges', () => {
    expect(step(calls, 5, 'newer')).toBeNull();
    expect(step(calls, 1, 'older')).toBeNull();
  });

  it('starts at the top row when nothing is playing or the call left the page', () => {
    expect(step(calls, null, 'older')?.id).toBe(5);
    expect(step(calls, 99, 'newer')?.id).toBe(5);
    expect(step([], null, 'newer')).toBeNull();
  });
});

describe('when a call ends or fails', () => {
  it('plays the newer call next only in continuous play', () => {
    expect(afterEnd(calls, 3, true, new Set())?.id).toBe(4);
    expect(afterEnd(calls, 3, false, new Set())).toBeNull();
    expect(afterEnd(calls, 5, true, new Set())).toBeNull();
  });

  it('passes over a newer call that failed before', () => {
    expect(afterEnd(calls, 2, true, new Set([3]))?.id).toBe(4);
  });

  it('passes over a call that fails, in the direction it was going, and over others that failed', () => {
    expect(afterFailure(calls, 3, 'newer', new Set([3]))?.id).toBe(4);
    expect(afterFailure(calls, 3, 'older', new Set([3, 2]))?.id).toBe(1);
    expect(afterFailure(calls, 2, 'older', new Set([2, 1]))).toBeNull();
  });
});

describe('the N and P keys', () => {
  const key = (init: KeyboardEventInit, target: Element = document.body) => {
    const event = new KeyboardEvent('keydown', init);
    Object.defineProperty(event, 'target', { value: target });
    return navKey(event);
  };

  it('map N to newer and P to older, either case', () => {
    expect(key({ key: 'n' })).toBe('newer');
    expect(key({ key: 'P' })).toBe('older');
    expect(key({ key: 'x' })).toBeNull();
  });

  it('are ignored with a modifier, on repeat, while typing and under a dialog', () => {
    expect(key({ key: 'p', metaKey: true })).toBeNull();
    expect(key({ key: 'p', ctrlKey: true })).toBeNull();
    expect(key({ key: 'n', repeat: true })).toBeNull();
    expect(key({ key: 'n' }, document.createElement('input'))).toBeNull();
    expect(key({ key: 'n' }, document.createElement('textarea'))).toBeNull();
    const editable = document.createElement('div');
    editable.setAttribute('contenteditable', 'true');
    document.body.append(editable);
    expect(key({ key: 'n' }, editable)).toBeNull();
    const dialog = document.createElement('dialog');
    dialog.setAttribute('open', '');
    document.body.append(dialog);
    expect(key({ key: 'n' })).toBeNull();
    dialog.remove();
    editable.remove();
  });
});
