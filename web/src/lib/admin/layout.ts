// What the "Layout" area shows, from `/api/admin/layout`: each ScannerPulse
// machine, each database machine's role and how far the standby is behind,
// the referee, the shared address, and the failover mode.

import { durationText } from '../status';

export type Mode = 'one' | 'hand' | 'referee' | 'unknown';

export interface LayoutMachine {
  host: string;
  version: string;
  seen_seconds_ago: number;
  /** Heard from within the last 45 seconds. */
  up: boolean;
  holds_shared: boolean;
  holds_lease: boolean;
}

export interface DatabaseMachine {
  host: string;
  /** Null when it does not answer (`error` says why). */
  role: 'primary' | 'former_primary' | 'standby' | null;
  error?: string;
  behind_bytes?: number | null;
  replayed_seconds_ago?: number | null;
}

export interface LayoutInfo {
  mode: Mode;
  first: string | null;
  second: string | null;
  shared: string | null;
  machines: LayoutMachine[];
  /** The machine that files and transcribes calls. */
  lease: string | null;
  database: DatabaseMachine[];
  referee: { host: string; answers: boolean; error?: string } | null;
  /** Run on the first machine; null on one machine alone. */
  switchover: string | null;
}

/** A machine that answers as a primary on an older timeline, back after a hand promotion. */
export const formerPrimaryText =
  'answers as a primary on an older timeline: run sudo /usr/local/bin/scannerpulse db rejoin there';

/** A standby this far behind needs a look. */
export const BEHIND_FLAG_BYTES = 16 * 1024 ** 2;

export function modeText(mode: Mode): string {
  switch (mode) {
    case 'one':
      return 'One machine: no failover.';
    case 'hand':
      return 'Two database machines: the standby is promoted by hand (sudo /usr/local/bin/scannerpulse db promote, on the standby).';
    case 'referee':
      return 'Two database machines: the referee promotes the standby by itself.';
    default:
      return "Database machines, whose plan this machine has not been given yet: a join, or second on the first machine, writes it.";
  }
}

export function bytesText(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`;
  const units = ['kB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

/** How far a standby is behind its primary, and whether that is fine. */
export function standbyText(node: Pick<DatabaseMachine, 'behind_bytes' | 'replayed_seconds_ago'>): {
  text: string;
  ok: boolean;
} {
  const bytes = node.behind_bytes ?? null;
  if (bytes === null) return { text: 'its distance is unknown: no primary answers', ok: false };
  const replayed =
    node.replayed_seconds_ago == null
      ? ''
      : ` (its last replayed write ${durationText(node.replayed_seconds_ago * 1000)} ago)`;
  const distance = bytes === 0 ? 'in step' : `${bytesText(bytes)} behind`;
  return { text: `${distance}${replayed}`, ok: bytes < BEHIND_FLAG_BYTES };
}

/** Whether a ScannerPulse machine answers. */
export function machineText(machine: Pick<LayoutMachine, 'up' | 'version' | 'seen_seconds_ago'>): {
  text: string;
  ok: boolean;
} {
  if (machine.up) return { text: `answering (version ${machine.version})`, ok: true };
  return { text: `not heard from for ${durationText(machine.seen_seconds_ago * 1000)}`, ok: false };
}
