import { SvelteSet } from 'svelte/reactivity';
import type { Call } from './api';
import { view } from './calls.svelte';
import { step, type Direction } from './playback';

/** The call in the player and the way the listener was moving through the list. */
export const player = $state({
  call: null as Call | null,
  direction: 'newer' as Direction,
  /** Sound is coming out: "Play new calls" waits for it to stop. */
  busy: false,
});

/** Calls played or failed in this visit; nothing is kept after the page closes. */
export const played = new SvelteSet<number>();
export const failed = new SvelteSet<number>();

export function play(call: Call, direction: Direction = 'newer') {
  player.call = call;
  player.direction = direction;
  played.add(call.id);
}

/** Plays the next row up or down; nothing at the page's edge. */
export function stepTo(direction: Direction) {
  const next = step(view.calls, player.call?.id ?? null, direction);
  if (next) play(next, direction);
}
