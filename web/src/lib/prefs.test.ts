import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DEFAULTS, applyTheme, loadPrefs, resetPref, savePref } from './prefs';

describe('prefs', () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.restoreAllMocks());

  it('gives the defaults when nothing is stored', () => {
    expect(loadPrefs()).toEqual(DEFAULTS);
  });

  it('keeps what the listener chose', () => {
    savePref('theme', 'dark');
    savePref('continuous', false);
    savePref('pageSize', 250);
    const prefs = loadPrefs();
    expect(prefs.theme).toBe('dark');
    expect(prefs.continuous).toBe(false);
    expect(prefs.pageSize).toBe(250);
  });

  it('ignores a stored value that is not valid', () => {
    localStorage.setItem('scannerpulse.pageSize', '7');
    localStorage.setItem('scannerpulse.theme', 'purple');
    localStorage.setItem('scannerpulse.smartSort', 'maybe');
    const prefs = loadPrefs();
    expect(prefs.pageSize).toBe(DEFAULTS.pageSize);
    expect(prefs.theme).toBe('auto');
    expect(prefs.smartSort).toBe(DEFAULTS.smartSort);
  });

  it('survives storage that is refused', () => {
    const get = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('denied');
    });
    const set = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('denied');
    });
    expect(loadPrefs()).toEqual(DEFAULTS);
    expect(() => savePref('theme', 'light')).not.toThrow();
  });

  it('keeps the column choice, even an empty one, and forgets it on reset', () => {
    expect(loadPrefs().columns).toBeNull();
    savePref('columns', ['time', 'id', 'text']);
    expect(loadPrefs().columns).toEqual(['time', 'id', 'text']);
    savePref('columns', []);
    expect(loadPrefs().columns).toEqual([]);
    resetPref('columns');
    expect(loadPrefs().columns).toBeNull();
  });

  it('saving no choice at all forgets the stored one, and is not "every column hidden"', () => {
    savePref('columns', ['time']);
    savePref('columns', null);
    expect(localStorage.getItem('scannerpulse.columns')).toBeNull();
    expect(loadPrefs().columns).toBeNull();
  });

  it('ignores a stored column choice that names none of our columns', () => {
    localStorage.setItem('scannerpulse.columns', '<b>,nothing');
    expect(loadPrefs().columns).toBeNull();
  });

  it('survives a reset when storage is refused', () => {
    vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => {
      throw new Error('denied');
    });
    expect(() => resetPref('columns')).not.toThrow();
  });

  it('sets the theme on the page, and auto leaves it to the device', () => {
    const root = document.createElement('html');
    applyTheme('dark', root);
    expect(root.dataset.theme).toBe('dark');
    applyTheme('auto', root);
    expect(root.dataset.theme).toBeUndefined();
  });
});
