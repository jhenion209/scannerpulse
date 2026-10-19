// The admin page's updates and the root helper (`/api/admin/updates`): the
// newest release, the helper on the first ScannerPulse machine, and the
// latest request an admin made of it (an update or a switchover).

export interface HelperRequest {
  id: number;
  kind: 'update' | 'switchover';
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
  requests: { update: HelperRequest | null; switchover: HelperRequest | null };
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

/** An admin may ask the helper now: it is up and no request waits or runs. */
export const canAsk = (info: UpdatesInfo) => info.helper?.up === true && !info.open;

/** A request's state in words. */
export function requestState(request: HelperRequest): { text: string; ok: boolean } {
  const what = request.kind === 'update' ? 'The update' : 'The switchover';
  if (request.started_at === null) return { text: `${what} waits for the root helper.`, ok: true };
  if (request.finished_at === null) return { text: `${what} runs on ${request.host}.`, ok: true };
  return request.ok
    ? { text: `${what} is done.`, ok: true }
    : { text: `${what} stopped: see its last line.`, ok: false };
}
