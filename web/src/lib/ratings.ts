// The stars this browser has given, kept in its own storage so the rows can
// show them. The server's record is the rating itself; this is only a
// convenience for showing it. Storage may be refused: callers fall back to
// no stars shown.

/** The most ratings kept; past it the oldest calls (lowest ids) are dropped. */
export const MAX_KEPT = 2000;

/** The stars kept by call id, read from stored text: anything malformed is dropped. */
export function parseMine(raw: string | null): Record<number, number> {
  const kept: Record<number, number> = {};
  if (!raw) return kept;
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return kept;
  }
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return kept;
  for (const [key, stars] of Object.entries(value)) {
    const id = Number(key);
    if (Number.isSafeInteger(id) && id > 0 && Number.isInteger(stars) && (stars as number) >= 1 && (stars as number) <= 5) {
      kept[id] = stars as number;
    }
  }
  return kept;
}

/** The text to store: at most `MAX_KEPT` calls, the newest. */
export function formatMine(mine: Record<number, number>): string {
  const ids = Object.keys(mine).map(Number).sort((a, b) => b - a).slice(0, MAX_KEPT);
  return JSON.stringify(Object.fromEntries(ids.map((id) => [id, mine[id]])));
}

/** What a tap on star `tapped` makes of the call's rating: tapping the star
 * already given clears it (null). */
export function nextStars(current: number | undefined, tapped: number): number | null {
  return current === tapped ? null : tapped;
}
