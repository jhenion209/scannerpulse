import { fetchFavorites, fetchFilterChoices, type Favorite, type FilterChoices } from './api';

/** How often the filter choices are read again: they are expensive for the
 * server, so never on each interaction. */
export const CHOICES_REFRESH_MS = 5 * 60_000;

export const choices = $state<{ filters: FilterChoices; favorites: Favorite[]; error: string | null }>({
  filters: { talkgroup_groups: [], systems: [], sites: [] },
  favorites: [],
  error: null,
});

async function refresh() {
  try {
    const [filters, favorites] = await Promise.all([fetchFilterChoices(), fetchFavorites()]);
    choices.filters = filters;
    choices.favorites = favorites;
    choices.error = null;
  } catch (error) {
    choices.error = error instanceof Error ? error.message : String(error);
  }
}

/** Reads the choices now and then on a timer; returns the stop. */
export function startChoices(): () => void {
  void refresh();
  const timer = setInterval(refresh, CHOICES_REFRESH_MS);
  return () => clearInterval(timer);
}
