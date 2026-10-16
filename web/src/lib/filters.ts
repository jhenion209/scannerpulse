// What the listener filters the list by, as the page's URL holds it, and the
// request it makes. The default is a rolling window: the last 24 hours,
// worked out at each request, never written into the URL.

export interface Filters {
  favorite: number | null;
  talkgroups: number[];
  system: number | null;
  /** Local date-times as `YYYY-MM-DDTHH:MM`, from the date pickers. */
  from: string | null;
  to: string | null;
  allDates: boolean;
  smart: boolean;
}

export const NO_FILTERS: Filters = {
  favorite: null,
  talkgroups: [],
  system: null,
  from: null,
  to: null,
  allDates: false,
  smart: false,
};

/** The rolling default window. */
export const ROLLING_HOURS = 24;

const id = (text: string | null): number | null => {
  if (text === null || !/^\d+$/.test(text)) return null;
  const n = Number(text);
  return Number.isSafeInteger(n) ? n : null;
};

const localTime = (text: string | null): string | null =>
  text !== null && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(text) ? text : null;

/** The filters a URL's query holds; anything malformed is left out. */
export function parseFilters(search: string): Filters {
  const params = new URLSearchParams(search);
  const talkgroups = (params.get('talkgroups') ?? '')
    .split(',')
    .map((part) => id(part.trim()))
    .filter((n): n is number => n !== null);
  return {
    favorite: id(params.get('favorite')),
    talkgroups: [...new Set(talkgroups)],
    system: id(params.get('system')),
    from: localTime(params.get('from')),
    to: localTime(params.get('to')),
    allDates: params.get('all') === '1',
    smart: params.get('smart') === '1',
  };
}

/** The query that reproduces the filters; the defaults are left out. */
export function filtersToSearch(filters: Filters): string {
  const params = new URLSearchParams();
  if (filters.favorite !== null) params.set('favorite', String(filters.favorite));
  if (filters.talkgroups.length) params.set('talkgroups', filters.talkgroups.join(','));
  if (filters.system !== null) params.set('system', String(filters.system));
  if (filters.allDates) params.set('all', '1');
  else {
    if (filters.from) params.set('from', filters.from);
    if (filters.to) params.set('to', filters.to);
  }
  if (filters.smart) params.set('smart', '1');
  const text = params.toString();
  return text ? `?${text}` : '';
}

/** The talkgroups a request asks for: the chosen favorite's and the chosen ones. */
export function talkgroupsAskedFor(
  filters: Filters,
  favorites: { id: number; talkgroups: number[] }[],
): number[] {
  const favorite = favorites.find((f) => f.id === filters.favorite);
  return [...new Set([...(favorite?.talkgroups ?? []), ...filters.talkgroups])].sort(
    (a, b) => a - b,
  );
}

export interface PageAsk {
  size: number;
  after?: string | null;
  /** False on a page turn: the total is known already. */
  count?: boolean;
}

/** The `/api/calls` query for the filters, at the instant `now`. */
export function listQuery(
  filters: Filters,
  favorites: { id: number; talkgroups: number[] }[],
  page: PageAsk,
  now: Date,
): URLSearchParams {
  const params = new URLSearchParams();
  const talkgroups = talkgroupsAskedFor(filters, favorites);
  if (talkgroups.length) params.set('talkgroups', talkgroups.join(','));
  if (filters.system !== null) params.set('system', String(filters.system));
  if (!filters.allDates) {
    const from = filters.from ? new Date(filters.from) : null;
    const to = filters.to ? new Date(filters.to) : null;
    if (from || to) {
      if (from && !Number.isNaN(from.getTime())) params.set('from', from.toISOString());
      if (to && !Number.isNaN(to.getTime())) params.set('until', to.toISOString());
    } else {
      params.set('from', new Date(now.getTime() - ROLLING_HOURS * 3600_000).toISOString());
    }
  }
  if (filters.smart) params.set('smart', 'true');
  params.set('size', String(page.size));
  if (page.after) params.set('after', page.after);
  if (page.count === false) params.set('count', 'false');
  return params;
}
