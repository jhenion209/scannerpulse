// Selection mode: the calls the listener ticked on the page on screen. It is a
// list of the calls chosen (nothing is chosen at first), and it never reaches
// past the page: a call that leaves the page leaves the choice.

export type PageState = 'none' | 'some' | 'all';

export const toggleOne = (chosen: number[], id: number) =>
  chosen.includes(id) ? chosen.filter((c) => c !== id) : [...chosen, id];

export function pageState(chosen: number[], page: number[]): PageState {
  const ticked = page.filter((id) => chosen.includes(id)).length;
  if (ticked === 0) return 'none';
  return ticked === page.length ? 'all' : 'some';
}

/** The header's box: tick the page, or clear it when it is all ticked. */
export const toggleAll = (chosen: number[], page: number[]) =>
  pageState(chosen, page) === 'all' ? [] : [...page];

export const keepOnPage = (chosen: number[], page: number[]) => chosen.filter((id) => page.includes(id));
