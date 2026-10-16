// The Transcription area's pure parts: finding a talkgroup in the list, the
// queue job's request, and the words for its preview.

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
  const calls = `${count.toLocaleString('en-US')} ${count === 1 ? 'call' : 'calls'}`;
  const floor = minMs > 0 ? ` (calls shorter than ${(minMs / 1000).toLocaleString('en-US')} s are left out)` : '';
  return count === 0 ? `No call to queue${floor}.` : `${calls} would be queued${floor}.`;
}
