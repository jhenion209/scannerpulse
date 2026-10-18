// What the "This machine" area shows, from `/api/admin/machine`, and the
// commands that change each part: the web service runs without root, so
// these are run on the machine itself.

import { durationText } from '../status';

export interface MachineCertificate {
  names?: string[];
  not_before?: string;
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

/** Let's Encrypt, when it gives the certificate: how the names are proved,
 * and the last request. */
export interface Acme {
  proof: 'web' | 'dns-cloudflare';
  names: string[];
  tried: string | null;
  renewed: string | null;
  error: string | null;
}

export interface MachineInfo {
  host: string;
  /** Where the database runs: one setup installed here (it keeps backups),
   * or one the person runs. */
  database: 'here' | 'theirs';
  certificate: MachineCertificate | null;
  acme: Acme | null;
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

/** A certificate's end: how far off it is, and whether it is near: within
 * 30 days, or for a short certificate (renewed with a third of its life
 * left) within a quarter of its life. */
export function certificateEnd(notAfter: string, now: Date, notBefore?: string): { text: string; ok: boolean } {
  const end = new Date(notAfter).getTime();
  const ms = end - now.getTime();
  if (ms <= 0) return { text: `ended ${durationText(-ms)} ago`, ok: false };
  const life = notBefore === undefined ? Infinity : end - new Date(notBefore).getTime();
  return { text: `ends in ${durationText(ms)}`, ok: ms > Math.min(30 * 86_400_000, life / 4) };
}

/** How Let's Encrypt is shown the names, in words. */
export const proofName = (proof: Acme['proof']) =>
  proof === 'web' ? 'over the web' : 'through DNS at Cloudflare';

/** Let's Encrypt's last request, and whether it needs a look. */
export function renewalText(acme: Acme, now: Date): { text: string; ok: boolean } {
  const ago = (at: string) => `${durationText(Math.max(0, now.getTime() - new Date(at).getTime()))} ago`;
  if (acme.error !== null && acme.tried !== null) {
    return { text: `the request ${ago(acme.tried)} failed: ${acme.error} (asked again within the hour)`, ok: false };
  }
  if (acme.tried !== null && (acme.renewed === null || new Date(acme.renewed) < new Date(acme.tried))) {
    // A request ends within ten minutes: one unanswered for longer is stuck.
    const stuck = now.getTime() - new Date(acme.tried).getTime() > 15 * 60_000;
    return { text: `asked ${ago(acme.tried)}, no answer yet`, ok: !stuck };
  }
  if (acme.renewed !== null) return { text: `renewed ${ago(acme.renewed)}`, ok: true };
  return { text: 'renewed when a third of its life is left', ok: true };
}

/** A kind of backup in words. */
export const kindName = (kind: string) => ({ pgbackrest: 'pgBackRest', dump: 'Dump' })[kind] ?? kind;

/** The command that changes each part, run on this machine. */
export const COMMANDS = {
  certificate: 'sudo /usr/local/bin/scannerpulse tls install /path/to/cert.pem /path/to/key.pem',
  ownCertificate: 'sudo /usr/local/bin/scannerpulse tls self',
  letsEncryptWeb: 'sudo /usr/local/bin/scannerpulse tls letsencrypt web site.example.org',
  // The token is asked for, never typed as an argument.
  letsEncryptDns:
    'sudo /usr/local/bin/scannerpulse tls letsencrypt dns-cloudflare site.example.org    (asks for the Cloudflare token)',
  // It asks for the address, which holds a token, so it is never typed as an argument.
  push: (kind: string) => `sudo /usr/local/bin/scannerpulse backups push ${kind}    (asks for the address; off removes it)`,
  location: 'sudo /usr/local/bin/scannerpulse backups location /path/to/folder',
};
