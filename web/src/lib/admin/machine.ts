// What the "This machine" area shows, from `/api/admin/machine`, and the
// commands that change each part: the web service runs without root, so
// these are run on the machine itself.

import { durationText } from '../status';

export interface MachineCertificate {
  names?: string[];
  not_after?: string;
  self_signed?: boolean;
  /** Why the certificate could not be read. */
  error?: string;
}

export interface BackupKind {
  /** When its newest backup finished; null when none has. */
  newest: string | null;
  /** A push address is set (the address itself is never shown). */
  push: boolean;
}

export interface MachineInfo {
  host: string;
  /** Where the database runs: one setup installed here (it keeps backups),
   * or one the person runs. */
  database: 'here' | 'theirs';
  certificate: MachineCertificate | null;
  /** Null when the database is one the person runs, with its own backups. */
  backups: { folder: string; kinds: Record<string, BackupKind> } | null;
}

/** The backups run every night: a kind's newest older than this needs a look. */
export const BACKUP_DUE_MS = 36 * 3600_000;

/** How old a kind's newest backup is, and whether that is fine. */
export function backupAge(newest: string | null, now: Date): { text: string; ok: boolean } {
  if (newest === null) return { text: 'none yet', ok: false };
  const ms = now.getTime() - new Date(newest).getTime();
  return { text: `${durationText(Math.max(0, ms))} ago`, ok: ms <= BACKUP_DUE_MS };
}

/** A certificate's end: how far off it is, and whether it is near. */
export function certificateEnd(notAfter: string, now: Date): { text: string; ok: boolean } {
  const ms = new Date(notAfter).getTime() - now.getTime();
  if (ms <= 0) return { text: `ended ${durationText(-ms)} ago`, ok: false };
  return { text: `ends in ${durationText(ms)}`, ok: ms > 30 * 86_400_000 };
}

/** A kind of backup in words. */
export const kindName = (kind: string) => ({ pgbackrest: 'pgBackRest', dump: 'Dump' })[kind] ?? kind;

/** The command that changes each part, run on this machine. */
export const COMMANDS = {
  certificate: 'sudo /usr/local/bin/scannerpulse tls install /path/to/cert.pem /path/to/key.pem',
  ownCertificate: 'sudo /usr/local/bin/scannerpulse tls self',
  // It asks for the address, which holds a token, so it is never typed as an argument.
  push: (kind: string) => `sudo /usr/local/bin/scannerpulse backups push ${kind}    (asks for the address; off removes it)`,
  location: 'sudo /usr/local/bin/scannerpulse backups location /path/to/folder',
};
