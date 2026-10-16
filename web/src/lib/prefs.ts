// The listener's preferences, kept in this browser. Storage may be refused
// (private windows, blocked site data): every read and write survives that
// and falls back to the default.

export type Theme = 'auto' | 'light' | 'dark';

export interface Prefs {
  theme: Theme;
  continuous: boolean;
  autoplayNew: boolean;
  pageSize: number;
  smartSort: boolean;
  showAllRows: boolean;
  railOpen: boolean;
}

export const DEFAULTS: Prefs = {
  theme: 'auto',
  continuous: true,
  autoplayNew: false,
  pageSize: 100,
  smartSort: false,
  showAllRows: false,
  railOpen: true,
};

/** The page sizes a listener may choose. */
export const PAGE_SIZES = [10, 25, 50, 100, 250, 500] as const;

const PREFIX = 'scannerpulse.';

function read(key: string): string | null {
  try {
    return localStorage.getItem(PREFIX + key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(PREFIX + key, value);
  } catch {
    // Not kept: the preference lasts until the page closes.
  }
}

/** Every preference, each one its stored value when valid, else the default. */
export function loadPrefs(): Prefs {
  const flag = (key: keyof Prefs, fallback: boolean) => {
    const value = read(key);
    return value === 'true' ? true : value === 'false' ? false : fallback;
  };
  const theme = read('theme');
  const size = Number(read('pageSize'));
  return {
    theme: theme === 'light' || theme === 'dark' ? theme : DEFAULTS.theme,
    continuous: flag('continuous', DEFAULTS.continuous),
    autoplayNew: flag('autoplayNew', DEFAULTS.autoplayNew),
    pageSize: (PAGE_SIZES as readonly number[]).includes(size) ? size : DEFAULTS.pageSize,
    smartSort: flag('smartSort', DEFAULTS.smartSort),
    showAllRows: flag('showAllRows', DEFAULTS.showAllRows),
    railOpen: flag('railOpen', DEFAULTS.railOpen),
  };
}

export function savePref<K extends keyof Prefs>(key: K, value: Prefs[K]): void {
  write(key, String(value));
}

/** Applies a theme to the page; `auto` lets the device decide. */
export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement): void {
  if (theme === 'auto') delete root.dataset.theme;
  else root.dataset.theme = theme;
}
