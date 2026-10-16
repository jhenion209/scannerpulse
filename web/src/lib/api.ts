// The calls of the sub-project 3 API that the main page uses.

export interface Call {
  id: number;
  started_at: string;
  duration_ms: number | null;
  talkgroup_id: number | null;
  talkgroup: number | null;
  talkgroup_name: string | null;
  system: string | null;
  site: string | null;
  radio_id: number | null;
  radio_alias: string | null;
  transcript: string | null;
  origin: string | null;
}

export interface CallPage {
  calls: Call[];
  next: string | null;
  total: number | null;
  total_capped: boolean;
}

export interface TalkgroupGroup {
  system: string;
  system_id: number;
  talkgroups: { id: number; number: number; name: string }[];
}

export interface FilterChoices {
  talkgroup_groups: TalkgroupGroup[];
  systems: { id: number; name: string }[];
  sites: { id: number; name: string }[];
}

export interface Favorite {
  id: number;
  name: string;
  talkgroups: number[];
}

/** An answer of 400 or more: the server's `{error, detail}`. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly error: string,
    readonly detail: string,
  ) {
    super(detail || error);
  }
}

async function getJson<T>(path: string, signal?: AbortSignal): Promise<T> {
  const response = await fetch(path, { signal, headers: { accept: 'application/json' } });
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new ApiError(response.status, body?.error ?? response.statusText, body?.detail ?? '');
  }
  return response.json() as Promise<T>;
}

export const fetchCalls = (query: URLSearchParams, signal?: AbortSignal) =>
  getJson<CallPage>(`/api/calls?${query}`, signal);
export const fetchFilterChoices = () => getJson<FilterChoices>('/api/filters');
export const fetchFavorites = () => getJson<Favorite[]>('/api/favorites');
