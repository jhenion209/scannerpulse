// The settings as the admin page edits them (`/api/admin/settings`). A value
// is typed as JSON (`true`, `30`, `["a", "b"]`), and plain words that are
// not JSON are taken as text, so a time zone is typed without quotes.

export interface Setting {
  key: string;
  /** Null when it is not stored (then `updated_at` is null too). */
  value: unknown;
  updated_at: string | null;
  /** False for a stored row that no part reads: it can only be removed. */
  known: boolean;
}

const isJson = (text: string) => {
  try {
    JSON.parse(text);
    return true;
  } catch {
    return false;
  }
};

/** The value a field's text names: its JSON, else the text itself; null when empty. */
export function parseValue(text: string): { value: unknown } | null {
  const trimmed = text.trim();
  if (trimmed === '') return null;
  return { value: isJson(trimmed) ? JSON.parse(trimmed) : trimmed };
}

/** A stored value as a field shows it: text that cannot be mistaken for JSON
 * without its quotes, anything else as JSON. */
export const showValue = (value: unknown) =>
  typeof value === 'string' && !isJson(value.trim()) ? value : JSON.stringify(value);

/** The settings whose key starts with `prefix`. */
export const withPrefix = (settings: Setting[], prefix: string) => settings.filter((s) => s.key.startsWith(prefix));
