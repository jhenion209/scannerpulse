// What the top bar's chips say, from `/api/status`: the number of calls, the
// transcription queue and when it should be done, how old the newest call is,
// the server's time in the site's zone, and maintenance.

/** The newest call is a warning when it is older than this. */
export const STALE_AFTER_MS = 15 * 60_000;

/** How often the status is read again. */
export const STATUS_REFRESH_MS = 30_000;

/** How old the newest call is at `serverNow`, never below zero; null when there is none. */
export function ageOf(newest: string | null, serverNow: Date): number | null {
  if (newest === null) return null;
  return Math.max(0, serverNow.getTime() - Date.parse(newest));
}

/** No call, or none for 15 minutes: something upstream may have stopped. */
export const isStale = (age: number | null) => age === null || age > STALE_AFTER_MS;

/** A duration in its largest unit, or its two largest from an hour up. */
export function durationText(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s} s`;
  const min = Math.floor(s / 60);
  if (min < 60) return `${min} min`;
  const h = Math.floor(min / 60);
  if (h < 24) return min % 60 ? `${h} h ${min % 60} min` : `${h} h`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d} d ${h % 24} h` : `${d} d`;
}

/** The queue's end, rounded up to a whole minute; empty when unknown. */
export const etaText = (seconds: number | null) =>
  seconds === null ? '' : `about ${durationText(Math.max(60, Math.ceil(seconds / 60) * 60) * 1000)}`;

export const callsText = (calls: number, estimated: boolean) =>
  `${estimated ? 'about ' : ''}${calls.toLocaleString('en-US')} ${calls === 1 ? 'call' : 'calls'}`;

/** `HH:MM:SS ZONE` in the site's zone; a zone the browser does not know shows UTC. */
export function clockText(now: Date, zone: string): string {
  const format = (timeZone: string) =>
    new Intl.DateTimeFormat('en-US', {
      timeZone, hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23', timeZoneName: 'short',
    }).format(now);
  try {
    return format(zone);
  } catch {
    return format('UTC');
  }
}

/** The later of two call times. */
export const newer = (a: string | null, b: string) => (a === null || Date.parse(b) > Date.parse(a) ? b : a);
