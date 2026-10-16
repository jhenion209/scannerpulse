// The calls of the sub-project 3 API that the main page uses.

import { nameFromHeader } from './exporting';

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

/** `/api/calls/<id>`: a call with its file's name. */
export interface CallDetail extends Call {
  file: string;
  labeled: boolean;
}

/** `/api/status`: what the top bar's chips show. */
export interface Status {
  maintenance: boolean;
  transcription: boolean;
  queue: number;
  eta_seconds: number | null;
  calls: number;
  calls_estimated: boolean;
  newest_call: string | null;
  server_time: string;
  time_zone: string;
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

/** The error an answer of 400 or more carries, in the server's words. */
export async function apiError(response: Response): Promise<ApiError> {
  const body = await response.json().catch(() => ({}));
  return new ApiError(response.status, body?.error ?? (response.statusText || `HTTP ${response.status}`), body?.detail ?? '');
}

async function getJson<T>(path: string, signal?: AbortSignal): Promise<T> {
  const response = await fetch(path, { signal, headers: { accept: 'application/json' } });
  if (!response.ok) throw await apiError(response);
  return response.json() as Promise<T>;
}

export const fetchCalls = (query: URLSearchParams, signal?: AbortSignal) =>
  getJson<CallPage>(`/api/calls?${query}`, signal);
export const fetchFilterChoices = () => getJson<FilterChoices>('/api/filters');
export const fetchFavorites = () => getJson<Favorite[]>('/api/favorites');
export const fetchStatus = () => getJson<Status>('/api/status');
export const fetchCall = (id: number) => getJson<CallDetail>(`/api/calls/${id}`);

/** Saves a person's text for a call (the public edit); throws the server's
 * refusal. An admin signed in on this browser saves under their own name:
 * the save then carries the session's token. */
export async function saveTranscript(id: number, text: string): Promise<{ id: number; transcript: string; origin: string }> {
  const headers: Record<string, string> = { 'content-type': 'application/json', accept: 'application/json' };
  const me = await fetch('/api/auth/me', { headers: { accept: 'application/json' } }).catch(() => null);
  if (me?.ok) headers['x-csrf-token'] = (await me.json()).csrf;
  const response = await fetch(`/api/calls/${id}/transcript`, {
    method: 'PATCH',
    headers,
    body: JSON.stringify({ text }),
  });
  if (!response.ok) throw await apiError(response);
  return response.json();
}

/** A file the server built, with what the listener should know about it. */
export interface Download {
  blob: Blob;
  name: string;
  note: string | null;
}

/** The Excel file of every match of the list's query. */
export async function fetchExcel(query: URLSearchParams): Promise<Download> {
  const response = await fetch(`/api/export/xlsx?${query}`);
  if (!response.ok) throw await apiError(response);
  const capped = response.headers.get('x-export-capped-at');
  return {
    blob: await response.blob(),
    name: nameFromHeader(response.headers.get('content-disposition'), 'ScannerPulse.xlsx'),
    note: capped ? `Only the newest ${Number(capped).toLocaleString('en-US')} calls fit in one file.` : null,
  };
}

/** The calls' audio joined into one file: every match of the filter, or the ids given. */
export async function fetchAudio(body: Record<string, unknown>): Promise<Download> {
  const response = await fetch('/api/export/audio', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw await apiError(response);
  const missing = Number(response.headers.get('x-export-missing') ?? 0);
  return {
    blob: await response.blob(),
    name: nameFromHeader(response.headers.get('content-disposition'), 'ScannerPulse.mp3'),
    note: missing ? `${missing} ${missing === 1 ? 'call' : 'calls'} could not be read and ${missing === 1 ? 'was' : 'were'} left out.` : null,
  };
}
