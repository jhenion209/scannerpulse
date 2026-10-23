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
  /** An import's offset, in minutes: its file names' times are moved by it. */
  offset_minutes: number | null;
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

/** The calls a purge takes from one location. */
export type PurgeHeld = { location: string; read_only: boolean; calls: number; bytes_about: number | null };

/** What purging every call before a day would do, as the server counted it. */
export type PurgePreview = {
  before: string;
  calls: number;
  held: PurgeHeld[];
  bytes_about: number | null;
  refused: string | null;
};

/** What a purge's count found, in words: a read-only location keeps its files. */
export function purgeText(p: PurgePreview): string {
  const space = p.bytes_about === null ? '' : ` (about ${sizeText(p.bytes_about)})`;
  const where = p.held
    .map((h) => `${h.calls.toLocaleString('en-US')} in ${h.location}${h.read_only ? ' (read-only: only their records go)' : ''}`)
    .join(', ');
  return `${callsWord(p.calls)} before ${p.before}${space} would be deleted for good: ${where}.`;
}

/** A purge's last button: red, naming what it does. */
export const purgeAction = (p: PurgePreview) => `Delete ${callsWord(p.calls)}`;

/** A purge's confirmation pop-up. */
export function purgeAsk(p: PurgePreview): { title: string; text: string; action: string } {
  return {
    title: `${purgeAction(p)} before ${p.before}?`,
    text: 'Their records go now, and their files are deleted for good; a read-only location keeps its files.',
    action: purgeAction(p),
  };
}

/** The confirmation pop-up's question, words and button. */
export function moveAsk(p: MovePreview): { title: string; text: string; action: string } {
  return {
    title: `${moveAction(p)}?`,
    text: `Each file is copied and checked, its call then points at ${p.to}, and the old file is deleted. Moving them back is a new move.`,
    action: `Move ${callsWord(p.calls)}`,
  };
}

/** Examples of each kind of file a review found. */
export interface Examples {
  no_date: string[];
  new: string[];
  copies: string[];
  unknown: string[];
  differ: string[];
  unreadable: string[];
}

/**
 * `GET /api/admin/storage/import/review/<job>`: what a review of an older
 * archive's folder found. The kinds of call are counted among the files
 * `read`, a sample of those `left`.
 */
export interface Review {
  name: string;
  folder: string;
  files: number;
  taken: number;
  no_date: number;
  bad_names: number;
  left: number;
  read: number;
  new: number;
  copies: number;
  unknown: number;
  differ: number;
  unreadable: number;
  before_purge: number;
  examples: Examples;
  copies_at: number[];
  /** The new calls an import at each of `OFFSETS` would make; the other counts are at the import's own offset. */
  new_at: number[];
  suggested: number | null;
  kept_offset: number | null;
  done: boolean;
}

/** The offsets at which a review counts the copies (`imports::OFFSETS`). */
export const OFFSETS = [-120, -60, 0, 60, 120];

/** An offset as the server reads it: `+01:00`, `-00:30`. */
export function offsetText(minutes: number): string {
  const sign = minutes < 0 ? '-' : '+';
  const abs = Math.abs(minutes);
  const two = (n: number) => String(n).padStart(2, '0');
  return `${sign}${two(Math.floor(abs / 60))}:${two(abs % 60)}`;
}

const number = (n: number) => n.toLocaleString('en-US');

/** A count made on the review's sample, scaled to the files left. */
function about(r: Review, n: number): string {
  if (n === 0 || r.read === 0 || r.read >= r.left) return number(n);
  return `about ${number(Math.round((n * r.left) / r.read))}`;
}

/** What a review found, one sentence a line. */
export function reviewLines(r: Review): string[] {
  let files = `${number(r.files)} mp3 ${r.files === 1 ? 'file' : 'files'} in ${r.folder}`;
  if (r.taken > 0) files += `; ${number(r.taken)} taken by an earlier run`;
  if (r.no_date > 0) files += `; ${number(r.no_date)} with no date and time in their names are skipped`;
  if (r.bad_names > 0) files += `; ${number(r.bad_names)} whose names are not UTF-8 are skipped`;
  const lines = [`${files}.`];
  if (r.read > 0) {
    const read = r.read >= r.left ? 'all were read' : `${number(r.read)} were read`;
    const copies = r.copies === 1 ? 'copy of a call' : 'copies of calls';
    lines.push(
      `Of the ${number(r.left)} left, ${read}: ${about(r, r.new)} new ${r.new === 1 ? 'call' : 'calls'}, ` +
        `${about(r, r.copies)} ${copies} already in the archive (skipped), ${about(r, r.unknown)} of systems not named yet (they wait), ` +
        `${about(r, r.unreadable)} unreadable (skipped).`,
    );
    if (r.differ > 0) {
      const differ = about(r, r.differ);
      lines.push(`${differ[0].toUpperCase()}${differ.slice(1)} new calls name their talkgroup otherwise: the names wait for approval on the Recorders area.`);
    }
    if (r.before_purge > 0) lines.push(`${about(r, r.before_purge)} are from before the day this import was purged before (skipped).`);
    lines.push(`Copies at each offset: ${OFFSETS.map((o, i) => `${offsetText(o)} ${number(r.copies_at[i] ?? 0)}`).join(', ')}.`);
  }
  if (r.kept_offset !== null) lines.push(`This import keeps its offset, ${offsetText(r.kept_offset)}.`);
  else if (r.suggested !== null) {
    lines.push(`The files are copies once their times are moved by ${offsetText(r.suggested)}: the older install’s clock or time zone was off.`);
    const i = OFFSETS.indexOf(r.suggested);
    if (i >= 0) {
      const [fresh, copies] = [r.new_at[i] ?? 0, r.copies_at[i] ?? 0];
      lines.push(
        `At ${offsetText(r.suggested)}: ${about(r, fresh)} new ${fresh === 1 ? 'call' : 'calls'}, ${about(r, copies)} ${copies === 1 ? 'copy of a call' : 'copies of calls'} already in the archive.`,
      );
    }
  }
  return lines;
}

/** An offset as a person types it (`+01:00`, `-00:30`, `0`), in minutes; null when it is not one. */
export function parseOffset(text: string): number | null {
  const found = /^([+-]?)(\d+):(\d{2})$/.exec(text.trim());
  if (text.trim() === '0') return 0;
  if (!found) return null;
  const [hours, minutes] = [Number(found[2]), Number(found[3])];
  if (minutes >= 60 || hours * 60 + minutes > 24 * 60) return null;
  return (found[1] === '-' ? -1 : 1) * (hours * 60 + minutes);
}

/**
 * The new calls an import at the offset typed would make, as the review
 * counted them: at the import's own offset (the one it keeps, or none; an
 * empty field), or at one it tried; null at any other.
 */
export function importCount(r: Review, offset: string): number | null {
  const own = r.kept_offset ?? 0;
  const minutes = offset.trim() === '' ? own : parseOffset(offset);
  if (minutes === null) return null;
  if (minutes === own) return r.new;
  const i = OFFSETS.indexOf(minutes);
  return i >= 0 ? (r.new_at[i] ?? 0) : null;
}

/** The import's last button, naming what it does at the offset typed; no number at an offset the review did not try. */
export function importAction(r: Review, name: string, offset: string): string {
  const counted = importCount(r, offset);
  if (counted === null) return `Import into ${name}`;
  const n = r.read >= r.left || r.read === 0 ? counted : Math.round((counted * r.left) / r.read);
  const calls = `${number(n)} ${n === 1 ? 'call' : 'calls'}`;
  return `Import ${r.read > 0 && r.read < r.left ? 'about ' : ''}${calls} into ${name}`;
}

/** `POST /api/admin/storage/import`: what starting the import did. */
export interface ImportStarted {
  job: number;
  location: number;
  source: number;
  offset: number;
  again: boolean;
}

/** `GET /api/admin/storage/consolidate`: what consolidating an import would do. */
export interface ConsolidatePreview {
  name: string;
  calls: number;
  bytes_about: number | null;
  free_bytes: number | null;
  refused: string | null;
}

/** What consolidating an import would do, in words. */
export function consolidateText(p: ConsolidatePreview): string {
  const space = p.bytes_about === null ? '' : ` (about ${sizeText(p.bytes_about)})`;
  const free = p.free_bytes === null ? '' : `, which has ${sizeText(p.free_bytes)} free`;
  return `${callsWord(p.calls)} of ${p.name}${space} would be copied into the main archive${free}. ${p.name}’s own files stay as they are.`;
}

/** A consolidation's last button. */
export const consolidateAction = (p: ConsolidatePreview) => `Copy ${callsWord(p.calls)} into the main archive`;

/** A consolidation's confirmation pop-up. */
export function consolidateAsk(p: ConsolidatePreview): { title: string; text: string; action: string } {
  return {
    title: `Copy ${callsWord(p.calls)} of ${p.name} into the main archive?`,
    text: `Each file is copied and checked, and its call then points at the copy. ${p.name}’s files stay as they are; once it holds no call, ${p.name} can be removed.`,
    action: `Copy ${callsWord(p.calls)}`,
  };
}
