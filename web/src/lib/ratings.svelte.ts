// The stars this browser has given, shared by the rows and the player bar.
// A tap shows at once and goes to the server; a refusal puts the old stars back.

import { rateCall } from './api';
import { tell } from './notice.svelte';
import { formatMine, nextStars, parseMine } from './ratings';

const KEY = 'scannerpulse.ratings';

function load(): Record<number, number> {
  try {
    return parseMine(localStorage.getItem(KEY));
  } catch {
    return {};
  }
}

/** The stars given from this browser, by call id. */
export const mine = $state({ stars: load() });

function put(id: number, stars: number | null) {
  if (stars === null) delete mine.stars[id];
  else mine.stars[id] = stars;
  try {
    localStorage.setItem(KEY, formatMine(mine.stars));
  } catch {
    // Storage refused: the stars still show until the page is closed.
  }
}

/** Gives call `id` the star tapped (or clears it, on the star already given). */
export async function rate(id: number, tapped: number) {
  const before = mine.stars[id];
  const next = nextStars(before, tapped);
  put(id, next);
  try {
    await rateCall(id, next);
  } catch (error) {
    put(id, before ?? null);
    tell(`The rating was not saved: ${error instanceof Error ? error.message : String(error)}`);
  }
}
