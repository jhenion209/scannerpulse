// The live feed's rules: which new calls join the list on screen, where, and
// how a transcript reaches its row. The connection is in feed.ts.

import type { Call } from './api';
import type { Filters } from './filters';

export type LiveEvent =
  | { type: 'call'; call: Call }
  | { type: 'transcript'; id: number; talkgroup_id: number | null; transcript: string | null; origin: string | null }
  | { type: 'resync' };

/** New calls join only the first page, while not paused, when the range
 * asked for has not ended. "All dates" counts as paused. */
export function liveApplies(filters: Filters, pageIndex: number, paused: boolean, now: Date): boolean {
  if (pageIndex !== 0 || paused || filters.allDates) return false;
  return !filters.to || new Date(filters.to).getTime() > now.getTime();
}

/** The server keeps the talkgroups; the system is the page's to check. */
export const wanted = (call: Call, talkgroups: number[], system: string | null) =>
  (talkgroups.length === 0 || (call.talkgroup_id !== null && talkgroups.includes(call.talkgroup_id))) &&
  (system === null || call.system === system);

/** The page with `call` in its place by time (newest first). No row is
 * dropped: the next page's cursor starts after the last row read, so a row
 * pushed off would be on neither page. Null when the call is shown already,
 * or is older than every row of a full page (a later page holds it). */
export function addCall(calls: Call[], call: Call, size: number): Call[] | null {
  if (calls.some((c) => c.id === call.id)) return null;
  // A call as new as the top row goes above it; one ingest filed late goes
  // below the newer calls.
  const time = Date.parse(call.started_at);
  const at = calls.findIndex((c) => Date.parse(c.started_at) <= time);
  if (at < 0) return calls.length >= size ? null : [...calls, call];
  return [...calls.slice(0, at), call, ...calls.slice(at)];
}

/** The page with the transcript written into its row; null when the call is not on it. */
export function setTranscript(calls: Call[], event: Extract<LiveEvent, { type: 'transcript' }>): Call[] | null {
  const at = calls.findIndex((c) => c.id === event.id);
  if (at < 0) return null;
  const rows = [...calls];
  rows[at] = { ...rows[at], transcript: event.transcript, origin: event.origin };
  return rows;
}

export function liveUrl(location: { protocol: string; host: string }, talkgroups: number[], since: number | null): string {
  const params = new URLSearchParams();
  if (talkgroups.length) params.set('talkgroups', talkgroups.join(','));
  if (since !== null) params.set('since', String(since));
  const query = params.toString();
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/live${query ? `?${query}` : ''}`;
}

/** The highest call id among `calls` and `known`. */
export const newestId = (calls: { id: number }[], known: number | null) =>
  calls.reduce<number | null>((max, c) => (max === null || c.id > max ? c.id : max), known);
