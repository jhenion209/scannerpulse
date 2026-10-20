// The Recorders area: the SDRTrunk folders ingest takes calls from, each
// watched, paused or taken in when asked.
import { counted } from '../format';
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

/** Its files filed and rejected since the site's midnight, and those waiting. */
export function todayText(r: Recorder): string {
  const waiting = r.waiting ? `, ${r.waiting.toLocaleString('en-US')} waiting` : '';
  return `${r.filed_today.toLocaleString('en-US')} filed, ${r.rejected_today.toLocaleString('en-US')} rejected${waiting}`;
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
