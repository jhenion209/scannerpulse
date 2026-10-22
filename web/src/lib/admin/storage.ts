// The Storage area: the main archive and the locations beside it, and
// moving a date range of calls into one.
import { zoned } from '../format';
import { durationText } from '../status';

/** What one host last found in a location. */
export interface Seen {
  host: string;
  ok: boolean;
  problem: string | null;
  free_bytes: number | null;
  total_bytes: number | null;
  checked_at: string;
}

export interface Location {
  id: number;
  name: string;
  root: string;
  mode: 'managed' | 'readonly';
  primary: boolean;
  hosts: Seen[];
  calls: number;
  first_call: string | null;
  last_call: string | null;
  bytes_about: number | null;
}

/** `GET /api/admin/storage`. */
export interface Storage {
  locations: Location[];
  hosts: string[];
  counted_at: string;
}

/** `GET /api/admin/storage/move`: what a move would do. */
export interface MovePreview {
  to: string;
  to_id: number;
  from_day: string;
  to_day: string;
  calls: number;
  held: { location: string; calls: number }[];
  bytes_about: number | null;
  free_bytes: number | null;
  refused: string | null;
}

/** A host's word older than this is no word (`storage::STALE_SECONDS`). */
export const STALE_MS = 5 * 60_000;

/** The word typed to start a move (`moves::CONFIRM`). */
export const CONFIRM = 'move';

/** A size as people read it, as the server writes it: `12.3 GB`. */
export function sizeText(bytes: number): string {
  const units = ['bytes', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return unit === 0 ? `${bytes} bytes` : `${value.toFixed(1)} ${units[unit]}`;
}

const callsWord = (n: number) => `${n.toLocaleString('en-US')} ${n === 1 ? 'call' : 'calls'}`;

/** Its calls: how many, from which day to which, and the space they use. */
export function callsText(l: Location, zone: string): string {
  if (l.calls === 0 || l.first_call === null || l.last_call === null) return 'No call';
  const day = (at: string) => zoned(new Date(at), zone).date;
  const space = l.bytes_about === null ? '' : ` · about ${sizeText(l.bytes_about)} used`;
  return `Calls from ${day(l.first_call)} to ${day(l.last_call)} · ${callsWord(l.calls)}${space}`;
}

/** The space left on its disk, by the newest word of a host that can use it. */
export function freeText(l: Location): string | null {
  const newest = l.hosts
    .filter((h) => h.ok && h.free_bytes !== null)
    .sort((a, b) => b.checked_at.localeCompare(a.checked_at))[0];
  return newest ? `${sizeText(newest.free_bytes!)} free` : null;
}

/** Whether each running host can use it: `host-a ✓`, or what is wrong. */
export function seenBy(l: Location, hosts: string[], now: Date): { host: string; ok: boolean; text: string }[] {
  return hosts.map((host) => {
    const seen = l.hosts.find((h) => h.host === host);
    if (!seen) return { host, ok: false, text: `${host} ✗ has not checked it yet` };
    if (!seen.ok) return { host, ok: false, text: `${host} ✗ cannot use it: ${seen.problem ?? 'it cannot be read'}` };
    const age = now.getTime() - new Date(seen.checked_at).getTime();
    if (age > STALE_MS) return { host, ok: false, text: `${host} ✗ has not checked it for ${durationText(age)}` };
    return { host, ok: true, text: `${host} ✓` };
  });
}

/** What the count found, in words. */
export function previewText(p: MovePreview): string {
  const space = p.bytes_about === null ? '' : ` (about ${sizeText(p.bytes_about)})`;
  const where = p.held.map((h) => `${h.calls.toLocaleString('en-US')} in ${h.location}`).join(', ');
  const free = p.free_bytes === null ? '' : ` ${p.to} has ${sizeText(p.free_bytes)} free.`;
  return `${callsWord(p.calls)}${space} of ${p.from_day} to ${p.to_day} would move into ${p.to}: ${where}.${free}`;
}

/** The last button's words, naming what it does. */
export const moveAction = (p: MovePreview) => `Move ${callsWord(p.calls)} to ${p.to}`;

/** The confirmation pop-up's question, words and button. */
export function moveAsk(p: MovePreview): { title: string; text: string; action: string } {
  return {
    title: `${moveAction(p)}?`,
    text: `Each file is copied and checked, its call then points at ${p.to}, and the old file is deleted. Moving them back is a new move.`,
    action: `Move ${callsWord(p.calls)}`,
  };
}
