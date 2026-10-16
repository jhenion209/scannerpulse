// The transcription queue draining while maintenance or a backlog runs: how
// far it has come down from the largest it was seen at.

/** The share drained, from 0 to 1, since the queue was at `largest`; null
 * while it has never held a call. */
export function drained(largest: number, now: number): number | null {
  if (largest <= 0) return null;
  return Math.min(1, Math.max(0, (largest - now) / largest));
}
