import { fetchFavorites, fetchFilterChoices, type Favorite, type FilterChoices } from './api';

/** How often the filter choices are read again: they are expensive for the
 * server, so never on each interaction. */
export const CHOICES_REFRESH_MS = 5 * 60_000;

export const choices = $state<{ filters: FilterChoices; favorites: Favorite[]; error: string | null }>({
  filters: { talkgroup_groups: [], systems: [], sites: [] },
  favorites: [],
  error: null,
});

/** After a failed read, one more try this soon, before the next timer. */
export const CHOICES_RETRY_MS = 30_000;

let retry: ReturnType<typeof setTimeout> | undefined;

async function refresh(again = true) {
  // Each read keeps what it got: a failed favorites read leaves good filter
  // choices in place, and the other way round.
  const [filters, favorites] = await Promise.allSettled([fetchFilterChoices(), fetchFavorites()]);
  if (filters.status === 'fulfilled') choices.filters = filters.value;
  if (favorites.status === 'fulfilled') choices.favorites = favorites.value;
  const failed = [filters, favorites].find((r) => r.status === 'rejected');
  if (failed) {
    const error = failed.reason;
    choices.error = error instanceof Error ? error.message : String(error);
    if (again) retry = setTimeout(() => void refresh(false), CHOICES_RETRY_MS);
  } else choices.error = null;
}

/** Reads the choices now and then on a timer; returns the stop. */
export function startChoices(): () => void {
  void refresh();
  const timer = setInterval(() => void refresh(), CHOICES_REFRESH_MS);
  return () => {
    clearInterval(timer);
    clearTimeout(retry);
  };
}
