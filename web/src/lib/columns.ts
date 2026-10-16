// The call list's columns and the listener's choice of them. The choice is a
// list of column ids kept in this browser; with none stored, the defaults for
// the screen's width apply.

import type { Call } from './api';
import { source } from './format';

export type ColumnId =
  | 'time' | 'talkgroup' | 'number' | 'system' | 'site' | 'source' | 'id' | 'length' | 'origin' | 'text';

/** Every column, in the order a table shows them. */
export const COLUMNS: { id: ColumnId; label: string }[] = [
  { id: 'time', label: 'Time' },
  { id: 'talkgroup', label: 'Talkgroup' },
  { id: 'number', label: 'Talkgroup number' },
  { id: 'system', label: 'System' },
  { id: 'site', label: 'Site' },
  { id: 'source', label: 'Source' },
  { id: 'id', label: 'Call ID' },
  { id: 'length', label: 'Length' },
  { id: 'origin', label: 'Text source' },
  { id: 'text', label: 'Text' },
];

const ORDER = COLUMNS.map((c) => c.id);

/** The columns that are plain values; a phone shows the chosen ones as
 * labelled values under the text. */
export const EXTRA_COLUMNS = ['number', 'system', 'site', 'source', 'id', 'origin'] as const;
export type ExtraId = (typeof EXTRA_COLUMNS)[number];

export const isExtra = (id: ColumnId): id is ExtraId => (EXTRA_COLUMNS as readonly string[]).includes(id);

/** Today's layout: a desk shows six columns; a phone leaves out site and source. */
export const defaultColumns = (phone: boolean): ColumnId[] =>
  phone ? ['time', 'talkgroup', 'length', 'text'] : ['time', 'talkgroup', 'site', 'source', 'length', 'text'];

/** The ids in table order, each once, with unknown names dropped. */
const tidy = (names: string[]): ColumnId[] => ORDER.filter((id) => names.includes(id));

/** The stored choice; null when nothing was stored or none of it is ours. An
 * empty text is a real choice: every column hidden. */
export function parseColumns(text: string | null): ColumnId[] | null {
  if (text === null) return null;
  if (text === '') return [];
  const chosen = tidy(text.split(',').map((name) => name.trim()));
  return chosen.length ? chosen : null;
}

export const formatColumns = (chosen: ColumnId[]): string => chosen.join(',');

/** The choice with `id` ticked or unticked, kept in table order. */
export const toggleColumn = (chosen: ColumnId[], id: ColumnId): ColumnId[] =>
  tidy(chosen.includes(id) ? chosen.filter((c) => c !== id) : [...chosen, id]);

/** An extra column's text for a call; empty when the call has no such value. */
export function extraText(call: Call, id: ExtraId): string {
  switch (id) {
    case 'site': return call.site ?? '';
    case 'source': return source(call);
    case 'system': return call.system ?? '';
    case 'number': return call.talkgroup === null ? '' : String(call.talkgroup);
    case 'id': return String(call.id);
    case 'origin': return call.origin ?? '';
  }
}
