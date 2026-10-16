import type { Call } from './api';

/** The length that fills the bar: about the 90th percentile of real calls,
 * so most calls do not crowd into the bar's first tenth. */
export const LENGTH_FULL_MS = 8000;

export const lengthPercent = (ms: number | null) =>
  ms === null ? 0 : Math.min(100, (ms / LENGTH_FULL_MS) * 100);

export const seconds = (ms: number | null) => (ms === null ? '' : `${(ms / 1000).toFixed(1)} s`);

const two = (n: number) => String(n).padStart(2, '0');

/** The call's local time; with its date when it is not from today. */
export function callTime(startedAt: string, now: Date): string {
  const t = new Date(startedAt);
  const time = `${two(t.getHours())}:${two(t.getMinutes())}:${two(t.getSeconds())}`;
  if (t.toDateString() === now.toDateString()) return time;
  return `${t.getFullYear()}-${two(t.getMonth() + 1)}-${two(t.getDate())} ${time}`;
}

export const source = (call: Pick<Call, 'radio_alias' | 'radio_id'>) =>
  call.radio_alias ?? (call.radio_id === null ? '' : String(call.radio_id));
