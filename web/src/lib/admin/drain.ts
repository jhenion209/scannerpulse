// The transcription queue draining while maintenance or a backlog runs: how
// far it has come down from the largest it was seen at.

/** The whole per cent drained since the queue was at `largest`, rounded down so
 * it reads 100 only once no call waits; null while the queue has never held a
 * call. Worked in integers: a share multiplied back by 100 loses a point to
 * float error (71 of 100 waiting would read 28, not 29). */
export function drainedPercent(largest: number, now: number): number | null {
  if (largest <= 0) return null;
  return Math.min(100, Math.max(0, Math.floor(((largest - now) * 100) / largest)));
}
