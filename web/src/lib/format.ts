import type { Call } from './api';

/** The length that fills the bar: about the 90th percentile of real calls,
 * so most calls do not crowd into the bar's first tenth. */
export const LENGTH_FULL_MS = 8000;

export const lengthPercent = (ms: number | null) =>
  ms === null ? 0 : Math.min(100, (ms / LENGTH_FULL_MS) * 100);

/** A count with its noun: `1 call`, `1,234 calls`. */
export const counted = (n: number, noun: string) => `${n.toLocaleString('en-US')} ${n === 1 ? noun : `${noun}s`}`;

export const seconds = (ms: number | null) => (ms === null ? '' : `${(ms / 1000).toFixed(1)} s`);

const two = (n: number) => String(n).padStart(2, '0');
const DAY_MS = 86_400_000;

// Every time on the pages is the site's (the `time_zone` setting), whatever
// the browser's own zone: one formatter per zone, made once.
const formats = new Map<string, Intl.DateTimeFormat>();

function formatter(zone: string): Intl.DateTimeFormat {
  let format = formats.get(zone);
  if (!format) {
    const make = (timeZone: string) =>
      new Intl.DateTimeFormat('en-US', {
        timeZone, year: 'numeric', month: '2-digit', day: '2-digit',
        hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23',
      });
    try {
      format = make(zone);
    } catch {
      // A zone this browser does not know: UTC, rather than no time at all.
      format = make('UTC');
    }
    formats.set(zone, format);
  }
  return format;
}

/** An instant's date and time in `zone`, as numbers. */
function fields(at: Date, zone: string) {
  const parts = formatter(zone).formatToParts(at);
  const get = (type: string) => Number(parts.find((p) => p.type === type)?.value ?? 0);
  return { year: get('year'), month: get('month'), day: get('day'), hour: get('hour'), minute: get('minute'), second: get('second') };
}

/** An instant's date (`YYYY-MM-DD`) and time (`HH:MM:SS`) in `zone`. */
export function zoned(at: Date, zone: string): { date: string; time: string } {
  const f = fields(at, zone);
  return { date: `${f.year}-${two(f.month)}-${two(f.day)}`, time: `${two(f.hour)}:${two(f.minute)}:${two(f.second)}` };
}

/** The call's time in `zone`; with its date when it is not from that day there. */
export function callTime(startedAt: string, now: Date, zone: string): string {
  const t = zoned(new Date(startedAt), zone);
  return t.date === zoned(now, zone).date ? t.time : `${t.date} ${t.time}`;
}

/** The instant a date picker's `YYYY-MM-DDTHH:MM` names in `zone`; null when
 * it names none. A time the clock skips forward over reads as the hour after
 * it, and one it repeats as the first of the two. */
export function zonedInstant(local: string, zone: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(local);
  if (!m) return null;
  const [year, month, day, hour, minute] = m.slice(1).map(Number);
  const wall = Date.UTC(year, month - 1, day, hour, minute);
  // A day or a time that does not exist (the 30th of February, 25:00).
  const named = new Date(wall);
  if (named.getUTCMonth() !== month - 1 || named.getUTCDate() !== day || hour > 23 || minute > 59) return null;
  // The zone's offset at an instant. A day before and a day after the time
  // asked for, the zone has the offsets on either side of any clock change.
  const offset = (at: number) => {
    const f = fields(new Date(at), zone);
    return Date.UTC(f.year, f.month - 1, f.day, f.hour, f.minute, f.second) - at;
  };
  const before = wall - offset(wall - DAY_MS);
  const after = wall - offset(wall + DAY_MS);
  const real = [before, after].filter((at) => at + offset(at) === wall);
  return new Date(real.length ? Math.min(...real) : before);
}

export const source = (call: Pick<Call, 'radio_alias' | 'radio_id'>) =>
  call.radio_alias ?? (call.radio_id === null ? '' : String(call.radio_id));
