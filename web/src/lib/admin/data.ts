// The Data area: renames, and the jobs that move or delete files, each started
// only with the talkgroup's name typed exactly.
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

/** The talkgroup a refused rename names: the same system's, with the name
 * exactly, the lowest number first, as the server picks it. */
export const takenBy = (list: AdminTalkgroup[], subject: AdminTalkgroup, name: string) =>
  list
    .filter((t) => t.system === subject.system && t.name === name && t.id !== subject.id)
    .sort((a, b) => a.number - b.number)[0];
