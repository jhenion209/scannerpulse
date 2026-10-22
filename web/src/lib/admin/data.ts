// The Data area: renames, and the jobs that move or delete files, each started
// only with the talkgroup's name typed exactly.
import { counted } from '../format';
import type { AdminTalkgroup } from './transcription';

/** A talkgroup as a preview names it. */
export interface Named {
  id: number;
  system: string;
  number: number;
  name: string;
}

/** A talkgroup whose files still carry an old name. */
export interface Uncorrected extends Named {
  files_name: string | null;
  calls: number;
}

export interface CorrectPreview {
  talkgroup: Named;
  calls: number;
  read_only: number;
}

export interface MergePreview {
  from: Named;
  into: Named;
  calls: number;
}

export interface PurgePreview {
  talkgroup: Named;
  calls: number;
  read_only: number;
  holding_path: string | null;
}

/** A started job. */
export interface Started {
  id: number;
  count: number;
}

/** `NAME (SYSTEM/NUMBER)`, as the server writes a talkgroup. */
export const written = (t: Named) => `${t.name} (${t.system}/${t.number})`;

/** What a correction moves, in words. */
export const correctText = (p: CorrectPreview) => {
  const stay = p.read_only === 1 ? '; 1 in a read-only location stays' : `; ${p.read_only.toLocaleString('en-US')} in read-only locations stay`;
  return `${counted(p.calls, 'file')} of ${written(p.talkgroup)} ${p.calls === 1 ? 'moves' : 'move'} to its name's folder${p.read_only > 0 ? stay : ''}.`;
};

/** What a merge moves, in words. */
export const mergeText = (p: MergePreview) =>
  `${counted(p.calls, 'call')} of ${written(p.from)} ${p.calls === 1 ? 'goes' : 'go'} to ${written(p.into)}; ${p.from.name} goes.`;

/** What a purge deletes, in words; the holding folder is set. A call in a
 * read-only location loses only its record: its file stays. */
export const purgeText = (p: PurgePreview) => {
  const held = p.calls - p.read_only;
  const kept =
    held === p.calls
      ? `${p.calls === 1 ? 'its file goes' : 'their files go'} to ${p.holding_path}`
      : `${counted(held, 'file')} ${held === 1 ? 'goes' : 'go'} to ${p.holding_path}`;
  const stay =
    p.read_only === 1
      ? '1 in a read-only location keeps its file'
      : `${p.read_only.toLocaleString('en-US')} in read-only locations keep their files`;
  const parts = [held > 0 ? kept : null, p.read_only > 0 ? stay : null].filter(Boolean);
  return `${counted(p.calls, 'call')} of ${written(p.talkgroup)} ${p.calls === 1 ? 'is' : 'are'} deleted; ${parts.join(', and ')}.`;
};

/** The talkgroup a refused rename names: the same system's, with the name
 * exactly, the lowest number first, as the server picks it. */
export const takenBy = (list: AdminTalkgroup[], subject: AdminTalkgroup, name: string) =>
  list
    .filter((t) => t.system === subject.system && t.name === name && t.id !== subject.id)
    .sort((a, b) => a.number - b.number)[0];
