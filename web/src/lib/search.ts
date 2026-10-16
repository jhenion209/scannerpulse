// The search panel's fields, what they ask the server, and how a link names
// a search (`search=` beside the list's own filters).

import type { Ask } from './calls.svelte';
import { NO_FILTERS } from './filters';

export interface SearchFields {
  text: string;
  talkgroup: number | null;
  system: number | null;
  /** Local date-times from the date fields; empty means no bound. */
  from: string;
  to: string;
  size: number;
}

/** The panel's request: its own words and filters, every date when no date
 * is given (never the list's rolling 24 hours), and an exact count; the
 * dates are read in the site's `zone`. */
export function searchAsk(fields: SearchFields, zone: string): Ask {
  return {
    filters: {
      ...NO_FILTERS,
      talkgroups: fields.talkgroup === null ? [] : [fields.talkgroup],
      system: fields.system,
      from: fields.from || null,
      to: fields.to || null,
      allDates: !fields.from && !fields.to,
    },
    favorites: [],
    size: fields.size,
    text: fields.text,
    exact: true,
    zone,
  };
}

/** The words a link searches for, or null. */
export function searchOf(search: string): string | null {
  const text = new URLSearchParams(search).get('search');
  return text?.trim() ? text : null;
}

/** The list's own query with the panel's words added, or taken away. */
export function withSearch(search: string, text: string | null): string {
  const params = new URLSearchParams(search);
  if (text?.trim()) params.set('search', text.trim());
  else params.delete('search');
  const out = params.toString();
  return out ? `?${out}` : '';
}
