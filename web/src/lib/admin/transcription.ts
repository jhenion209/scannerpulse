// The Transcription area's pure parts: finding and sorting the talkgroups in
// the list, the queue job's request, and the words for its preview.
import { counted } from '../format';

/** A talkgroup as `/api/admin/talkgroups` gives it. */
export interface AdminTalkgroup {
  id: number;
  system: string;
  number: number;
  name: string;
  /** New calls of the talkgroup are transcribed. */
  transcribe: boolean;
  /** Its archived files carry its name (false after a rename, until corrected). */
  files_correct: boolean;
}

/** The talkgroups whose name, number or system matches `text`; all when it is empty. */
export function matchTalkgroups(list: AdminTalkgroup[], text: string): AdminTalkgroup[] {
  const needle = text.trim().toLowerCase();
  if (!needle) return list;
  return list.filter(
    (t) => t.name.toLowerCase().includes(needle) || String(t.number).startsWith(needle) || t.system.toLowerCase() === needle,
  );
}

/** A column of the talkgroup table that sorts. */
export type TalkgroupColumn = 'system' | 'number' | 'name' | 'transcribe';

/** The table's order: the column and its direction. */
export interface TalkgroupSort {
  column: TalkgroupColumn;
  descending: boolean;
}

const byText = (a: string, b: string) => a.localeCompare(b, 'en', { sensitivity: 'base' });

/** The talkgroups in the table's order, as a new list. Numbers sort by value,
 * text without regard to case, and ascending puts the talkgroups that are on
 * first; equal rows follow system, then number, ascending. */
export function sortTalkgroups(list: AdminTalkgroup[], sort: TalkgroupSort): AdminTalkgroup[] {
  const key = (a: AdminTalkgroup, b: AdminTalkgroup): number => {
    switch (sort.column) {
      case 'system': return byText(a.system, b.system);
      case 'number': return a.number - b.number;
      case 'name': return byText(a.name, b.name);
      case 'transcribe': return Number(b.transcribe) - Number(a.transcribe);
    }
  };
  return [...list].sort(
    (a, b) => (sort.descending ? -key(a, b) : key(a, b)) || byText(a.system, b.system) || a.number - b.number,
  );
}

/** The order after a click on `column`'s header: the same column flips, another starts ascending. */
export const nextSort = (sort: TalkgroupSort, column: TalkgroupColumn): TalkgroupSort =>
  sort.column === column ? { column, descending: !sort.descending } : { column, descending: false };

/** The queue job's fields: one talkgroup (null: the backlog of every talkgroup
 * whose transcription is on) and the site's days, `YYYY-MM-DD` or empty. */
export interface QueueFields {
  talkgroup: number | null;
  from: string;
  to: string;
}

/** The fields as the preview's query and the job's body share them. */
export function queueBody(fields: QueueFields): { talkgroup_id?: number; from?: string; to?: string } {
  const body: { talkgroup_id?: number; from?: string; to?: string } = {};
  if (fields.talkgroup !== null) body.talkgroup_id = fields.talkgroup;
  if (fields.from) body.from = fields.from;
  if (fields.to) body.to = fields.to;
  return body;
}

/** `GET /api/admin/jobs/queue`'s query for the fields. */
export const queueQuery = (fields: QueueFields) =>
  new URLSearchParams(Object.entries(queueBody(fields)).map(([k, v]) => [k, String(v)]));

/** What the preview found, in words. */
export function previewText(count: number, minMs: number): string {
  const floor = minMs > 0 ? ` (calls shorter than ${(minMs / 1000).toLocaleString('en-US')} s are left out)` : '';
  return count === 0 ? `No call to queue${floor}.` : `${counted(count, 'call')} would be queued${floor}.`;
}

/** What putting the failed calls back did, in words. */
export const retriedText = (n: number) =>
  n === 0 ? 'No failed call to try again.' : `${counted(n, 'failed call')} ${n === 1 ? 'is' : 'are'} back in the queue.`;
