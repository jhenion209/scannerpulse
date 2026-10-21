// The Recorders area: the SDRTrunk folders ingest takes calls from, each
// watched, paused or taken in when asked; a recorder's preview; and what
// waits for the administrator's decision.
import { callTime, counted } from '../format';
import { durationText } from '../status';

export type RecorderState = 'watched' | 'paused' | 'asked';

/** What one host's watcher last found in a recorder's folder. */
export interface FolderHealth {
  host: string;
  ok: boolean;
  problem: string | null;
  files: number | null;
  checked_at: string;
}

export interface Recorder {
  id: number;
  name: string;
  folder: string;
  state: RecorderState;
  bringing_in: boolean;
  last_file: string | null;
  waiting: number;
  filed_today: number;
  rejected_today: number;
  staged_today: number;
  duplicates_today: number;
  hosts: FolderHealth[];
}

/** `GET /api/admin/recorders`: the recorders, and the hosts whose ingest runs. */
export interface Recorders {
  recorders: Recorder[];
  hosts: string[];
}

export type Change = 'watch' | 'ask' | 'pause' | 'bring-in' | 'remove';

/** A host's word on a folder older than this is no word: its ingest has
 * stopped listing it. Ingest lists every folder every few seconds. */
export const STALE_MS = 10 * 60_000;

/** How its files are taken in, in words. */
export function stateText(r: Recorder): string {
  switch (r.state) {
    case 'watched':
      return 'Watched';
    case 'paused':
      return 'Paused';
    case 'asked':
      return r.bringing_in ? 'Bringing in' : 'When asked';
  }
}

/** The changes a recorder's card offers, Remove last. */
export function changes(r: Recorder): Change[] {
  switch (r.state) {
    case 'watched':
      return ['pause', 'ask', 'remove'];
    case 'paused':
      return ['watch', 'ask', 'remove'];
    case 'asked':
      return ['bring-in', 'watch', 'remove'];
  }
}

export const CHANGE_LABEL: Record<Change, string> = {
  watch: 'Watch',
  ask: 'Only when asked',
  pause: 'Pause',
  'bring-in': 'Bring in now',
  remove: 'Remove…',
};

/** What a change did, for the notice. */
export function changedText(name: string, change: Change): string {
  switch (change) {
    case 'watch':
      return `${name} is watched: its files are taken as they arrive.`;
    case 'ask':
      return `${name} is taken in when asked: its files wait in its folder.`;
    case 'pause':
      return `${name} is paused: its files wait in its folder.`;
    case 'bring-in':
      return `${name}'s files are brought in: those its folder holds now are filed within a few seconds.`;
    case 'remove':
      return `${name} is removed: its files stay in its folder, its calls in the archive.`;
  }
}

/** Its files filed, rejected, removed as duplicates and staged since the
 * site's midnight, and those waiting. */
export function todayText(r: Recorder): string {
  const duplicates = r.duplicates_today ? `, ${counted(r.duplicates_today, 'duplicate')} removed` : '';
  const staged = r.staged_today ? `, ${r.staged_today.toLocaleString('en-US')} staged` : '';
  const waiting = r.waiting ? `, ${r.waiting.toLocaleString('en-US')} waiting` : '';
  return `${r.filed_today.toLocaleString('en-US')} filed, ${r.rejected_today.toLocaleString('en-US')} rejected${duplicates}${staged}${waiting}`;
}

/** When its newest file arrived. */
export const lastText = (r: Recorder, now: Date) =>
  r.last_file === null ? 'never' : `${durationText(Math.max(0, now.getTime() - new Date(r.last_file).getTime()))} ago`;

/** The files a recorder taken in when asked holds now, by the newest count
 * any host made; null before any host counted them. */
export function inFolder(r: Recorder): string | null {
  const newest = r.hosts
    .filter((h) => h.ok && h.files !== null)
    .sort((a, b) => b.checked_at.localeCompare(a.checked_at))[0];
  return newest ? `${counted(newest.files!, 'file')} in its folder` : null;
}

/** For each running host that cannot read the folder, has not listed it
 * yet, or has stopped listing it: what is wrong. A paused recorder is listed
 * by no host. */
export function folderProblems(r: Recorder, hosts: string[], now: Date): string[] {
  if (r.state === 'paused') return [];
  return hosts.flatMap((host) => {
    const seen = r.hosts.find((h) => h.host === host);
    if (!seen) return [`${host} has not listed the folder yet.`];
    if (!seen.ok) return [`${host} cannot read it: ${seen.problem ?? 'it cannot be listed'}.`];
    const age = now.getTime() - new Date(seen.checked_at).getTime();
    if (age > STALE_MS) return [`${host} has not listed it for ${durationText(age)}.`];
    return [];
  });
}

/** What filing would do with a file (`preview::Verdict`). */
export type Verdict =
  | { kind: 'filed' }
  | { kind: 'new_talkgroup' }
  | { kind: 'named'; current: string }
  | { kind: 'proposal'; current: string }
  | { kind: 'staged' }
  | { kind: 'duplicate'; call: number; recorder: string }
  | { kind: 'replaces'; call: number; recorder: string }
  | { kind: 'rejected'; reason: string }
  | { kind: 'unreadable'; reason: string };

export interface Counts {
  filed: number;
  new_talkgroup: number;
  named: number;
  proposal: number;
  staged: number;
  duplicate: number;
  replaces: number;
  rejected: number;
  unreadable: number;
}

/** A file of the sample, as the reader reads it. */
export interface Sampled {
  rel_path: string;
  started_at: string | null;
  system: string | null;
  site: string | null;
  talkgroup: number | null;
  talkgroup_name: string | null;
  radio_id: number | null;
  radio_alias: string | null;
  duration_ms: number | null;
  verdict: Verdict;
}

/** `GET /api/admin/recorders/<id>/preview`. */
export interface Preview {
  recorder: string;
  files: number;
  counts: Counts;
  sample: Sampled[];
}

/** The preview's counts, in words: the files filed first, with what they bring. */
export function previewText(p: Preview): string {
  if (p.files === 0) return 'Its folder holds no audio file.';
  const c = p.counts;
  const read = p.sample.length === p.files ? 'all read' : `${p.sample.length.toLocaleString('en-US')} read, spread over it`;
  const brings = [
    c.new_talkgroup && `${c.new_talkgroup} under a new talkgroup`,
    c.named && `${c.named} naming a placeholder`,
    c.proposal && `${c.proposal} with a name to approve`,
  ].filter(Boolean);
  const filed = c.filed + c.new_talkgroup + c.named + c.proposal;
  const parts = [
    filed && `${filed} filed${brings.length ? ` (${brings.join(', ')})` : ''}`,
    c.replaces && `${c.replaces} replacing a shorter recording`,
    c.duplicate && `${counted(c.duplicate, 'duplicate')} deleted`,
    c.staged && `${c.staged} staged`,
    c.rejected && `${c.rejected} rejected`,
    c.unreadable && `${c.unreadable} cannot be read now`,
  ].filter(Boolean);
  return `${counted(p.files, 'file')} in its folder, ${read}: ${parts.join(', ')}.`;
}

/** What filing would do with one file, in words. */
export function verdictText(v: Verdict): string {
  switch (v.kind) {
    case 'filed':
      return 'Filed';
    case 'new_talkgroup':
      return 'Filed under a new talkgroup';
    case 'named':
      return `Filed; names the talkgroup now called ${v.current}`;
    case 'proposal':
      return `Filed under ${v.current}; its name waits for your approval`;
    case 'staged':
      return 'Staged: its system is unknown';
    case 'duplicate':
      return `Deleted: a copy of call ${v.call} from ${v.recorder}, which keeps its recording`;
    case 'replaces':
      return `Replaces call ${v.call}'s recording from ${v.recorder}: a longer copy`;
    case 'rejected':
      return `Rejected: ${v.reason}`;
    case 'unreadable':
      return `Cannot be read now: ${v.reason}`;
  }
}

/** A talkgroup name a recorder sent that differs from the stored one. */
export interface Proposal {
  talkgroup_id: number;
  system: string;
  number: number;
  current: string;
  proposed: string;
  recorders: string[];
  calls: number;
  first_seen: string;
  last_seen: string;
}

/** A system name no system has; its files wait in the staging folder. */
export interface Unknown {
  system: string;
  files: number;
  recorders: string[];
  first_seen: string;
  last_seen: string;
  /** Some of its talkgroups, `<number> <name>`. */
  talkgroups: string[];
}

/** `GET /api/admin/names`: what waits for a decision. */
export interface Waiting {
  waiting: number;
  unknown: Unknown[];
  proposals: Proposal[];
  /** The systems an unknown name may be the same as. */
  systems: string[];
}

/** When a name was first and last seen, in the site's time. */
export const seenText = (first: string, last: string, now: Date, zone: string) =>
  `${callTime(first, now, zone)} – ${callTime(last, now, zone)}`;

export type SystemDecision = 'new-system' | 'map' | 'discard';
export type NameDecision = 'approve' | 'dismiss';

/** What a decision on an unknown system name did, for the notice. */
export function systemDecidedText(system: string, decision: SystemDecision, done: { to?: string; discarded?: number } = {}): string {
  switch (decision) {
    case 'new-system':
      return `${system} is a new system: its staged files are filed within a few seconds.`;
    case 'map':
      return `${system} is ${done.to} from now on: its staged files are filed under it within a few seconds, and wait for a correction on the Data area.`;
    case 'discard': {
      const n = done.discarded ?? 0;
      return `${counted(n, 'staged file')} of ${system} ${n === 1 ? 'is' : 'are'} deleted.`;
    }
  }
}

/** What a decision on a proposed name did, for the notice. */
export function nameDecidedText(p: Pick<Proposal, 'system' | 'number' | 'proposed'>, decision: NameDecision): string {
  return decision === 'approve'
    ? `${p.system} ${p.number} is ${p.proposed}; its files keep the old name until they are corrected on the Data area.`
    : `${p.proposed} is dismissed for ${p.system} ${p.number}: that spelling is ignored from now on.`;
}
