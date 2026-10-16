// A click on a call's row plays it, except on the row's own controls (its
// play edge, More, the actions menu, the selection box), which keep their
// own clicks, and except the click that ends selecting some of its text.

/** The row's controls: a click on or inside one is theirs, not the row's. */
const CONTROLS = 'button, a, input, select, textarea, label, summary, details';

/** Whether a click on `target` plays the row's call; `selected` is the text
 * the page has selected when the click lands. */
export function rowClickPlays(target: EventTarget | null, selected: string): boolean {
  if (selected !== '' || !(target instanceof Element)) return false;
  return target.closest(CONTROLS) === null;
}
