// The admin page's updates and the root helper (`/api/admin/updates`): the
// newest release, the helper on the first ScannerPulse machine, and the
// latest request of each kind an admin made of it (an update, a switchover,
// or a step of the conversion to high availability).

/** What an admin may ask the root helper for, and the request in words. */
export const HELPER_KINDS = {
  update: 'The update',
  switchover: 'The switchover',
  archive_check: 'The check of the copy',
  archive_switch: "The archive's switch",
  db_move: "The database's move",
  db_switch: "The database's switch",
  db_retire: "The old database's removal",
  second: "The second machine's addition",
} as const;

export type HelperKind = keyof typeof HELPER_KINDS;

/** The function an admin needs to ask for it: every step of the conversion
 * is `convert`. */
export const helperFunction = (kind: HelperKind) => (kind === 'update' || kind === 'switchover' ? kind : 'convert');

export interface HelperRequest {
  id: number;
  kind: HelperKind;
  requested_by: string;
  requested_at: string;
  started_at: string | null;
  finished_at: string | null;
  host: string | null;
  ok: boolean | null;
  /** What the helper said doing it, a line at a time. */
  said: string;
}

export interface UpdatesInfo {
  /** The version this web service runs. */
  running: string;
  machines: { host: string; version: string }[];
  /** The newest release the helper's daily check found; null before any. */
  newest: { version: string; newer: boolean; notes: string; checked_at: string } | null;
  /** Whether the daily check is on (`updates.check`). */
  check: boolean;
  helper: { host: string; version: string; seen_seconds_ago: number; up: boolean } | null;
  /** The newest request of each kind. */
  requests: Partial<Record<HelperKind, HelperRequest | null>>;
  /** A request of either kind waits or runs: another must wait. */
  open: boolean;
}

/** The newest release in words, and whether an update is available. */
export function newestText(info: UpdatesInfo): { text: string; available: boolean } {
  const newest = info.newest;
  if (newest === null) {
    return {
      text: info.check
        ? 'No release has been looked for yet: the root helper looks once a day.'
        : 'The daily look for a newer release is off (Settings: updates.check).',
      available: false,
    };
  }
  if (newest.newer) {
    return { text: `Update available: ${newest.version} (this site runs ${info.running}).`, available: true };
  }
  return { text: `Up to date: ${info.running} is the newest release.`, available: false };
}

/** The root helper in words, and whether it is up. */
export function helperText(info: UpdatesInfo): { text: string; ok: boolean } {
  const helper = info.helper;
  if (helper === null) {
    return {
      text: 'The root helper has never run: on the first ScannerPulse machine, run sudo /usr/local/bin/scannerpulse agent install once.',
      ok: false,
    };
  }
  if (!helper.up) {
    return {
      text: `The root helper on ${helper.host} has not been heard from for ${helper.seen_seconds_ago} s: is scannerpulse-agent running there?`,
      ok: false,
    };
  }
  return { text: `The root helper runs on ${helper.host} (${helper.version}).`, ok: true };
}

/** The heading of the newest request of `kind`: "The latest update". */
export const latestText = (kind: HelperKind) => `The latest ${HELPER_KINDS[kind].replace(/^The /, '')}`;

/** An admin may ask the helper now: it is up and no request waits or runs. */
export const canAsk = (info: UpdatesInfo) => info.helper?.up === true && !info.open;

/** A request's state in words. */
export function requestState(request: HelperRequest): { text: string; ok: boolean } {
  const what = HELPER_KINDS[request.kind] ?? 'The request';
  if (request.started_at === null) return { text: `${what} waits for the root helper.`, ok: true };
  if (request.finished_at === null) return { text: `${what} runs on ${request.host}.`, ok: true };
  return request.ok
    ? { text: `${what} is done.`, ok: true }
    : { text: `${what} stopped: see its last line.`, ok: false };
}
