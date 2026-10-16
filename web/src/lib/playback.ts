// The playback rules. Rows are newest first, so "newer" is the row above:
// the direction of continuous play and of N and ▲. Stepping stops at the
// page's edges; it never turns the page.

export type Direction = 'newer' | 'older';

/** The row one step from `id`; the top row when `id` is not on the page. */
export function step<T extends { id: number }>(calls: T[], id: number | null, direction: Direction): T | null {
  const at = id === null ? -1 : calls.findIndex((c) => c.id === id);
  if (at < 0) return calls[0] ?? null;
  return calls[direction === 'newer' ? at - 1 : at + 1] ?? null;
}

/** What plays when `id` ends: in continuous play, the next newer call that
 * has not failed. Nothing when `id` has left the page: the list is another
 * one now, and its top row is not the call that follows. */
export const afterEnd = <T extends { id: number }>(calls: T[], id: number, continuous: boolean, failed: Set<number>) =>
  continuous && calls.some((c) => c.id === id) ? afterFailure(calls, id, 'newer', failed) : null;

/** What plays when `id` fails: the next call the same way that has not failed. */
export function afterFailure<T extends { id: number }>(
  calls: T[],
  id: number,
  direction: Direction,
  failed: Set<number>,
): T | null {
  let next = step(calls, id, direction);
  while (next && failed.has(next.id)) next = step(calls, next.id, direction);
  return next;
}

/** The step a key asks for, or null when the key is not for the list. */
export function navKey(event: KeyboardEvent): Direction | null {
  if (event.repeat || event.ctrlKey || event.metaKey || event.altKey) return null;
  const target = event.target as Element | null;
  if (target?.closest?.('input, textarea, select, [contenteditable]:not([contenteditable="false"])')) return null;
  if (document.querySelector('dialog[open]')) return null;
  const key = event.key.toLowerCase();
  return key === 'n' ? 'newer' : key === 'p' ? 'older' : null;
}
