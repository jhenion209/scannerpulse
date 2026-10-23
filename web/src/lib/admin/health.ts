// What the Status area shows, from `/api/admin/status`, and how it reads
// out: the lease, the checks to make after a failover, a call's state.

import { durationText } from '../status';

/** A machine or recorder whose clock needs a look, and what is wrong. */
export interface ClockWarning {
  subject: string;
  text: string;
}

export interface Heartbeat {
  host: string;
  /** Seconds since it was last renewed. */
  age: number;
  /** Seconds until it expires; negative when it has. */
  expires_in: number;
}

export interface AdminStatus {
  maintenance: boolean;
  lease: { valid: boolean; holder: string | null; expires_in: number; heartbeats: Heartbeat[] };
  ingest: {
    waiting: number;
    rejected: number;
    recent_rejected: { source: string; rel_path: string; reason: string | null; seen_at: string }[];
  };
  queue: { queued: number; due: number; failed: number; last_minute: number };
  /** Each storage location a running host cannot use. */
  locations: { location: string; host: string; problem: string | null }[];
  /** Each clock to look at, by name. An older server sends none. */
  clocks?: ClockWarning[];
  failed: { call_id: number; started_at: string; attempts: number; last_error: string | null }[];
  servers: { url: string; cooling_until: string | null; failed_rounds: number; last_error: string | null; updated_at: string | null }[];
  totals: { calls_about: number | null; transcripts_about: number | null };
  recent_calls: {
    id: number;
    started_at: string;
    system: string | null;
    talkgroup: number | null;
    talkgroup_name: string | null;
    transcript_state: string;
  }[];
  database: {
    primary: boolean;
    version: string;
    address: string | null;
    port: number | null;
    standbys: { name: string | null; address: string | null; state: string | null; sync_state: string | null; replay_lag_seconds: number | null }[];
  };
  /** Null when there is no referee. */
  referee: { host: string; answers: boolean; error?: string } | null;
  failover: {
    lease_held: boolean;
    jobs_running: number;
    jobs_stuck: number;
    calls_failed: number;
    calls_filed_twice: number;
    files_left_in_sources: number;
  };
}

/** How often the Status area is read again. */
export const STATUS_EVERY_MS = 15_000;

/** Who holds the processing lease, and for how long. */
export function leaseText(lease: AdminStatus['lease']): string {
  if (lease.holder === null) return 'Ingest has never run against this database.';
  if (lease.holder === '') return 'No host holds the lease: the last holder let it go.';
  if (!lease.valid) return `${lease.holder} held the lease; it ran out ${durationText(-lease.expires_in * 1000)} ago.`;
  return `${lease.holder} holds the lease, for ${durationText(lease.expires_in * 1000)} more unless renewed.`;
}

export interface Check {
  label: string;
  value: string;
  ok: boolean;
}

/** The checks to make after a failover: each one is fine, or needs a look. */
export function failoverChecks(f: AdminStatus['failover']): Check[] {
  const none = (label: string, n: number): Check => ({ label, value: n.toLocaleString('en-US'), ok: n === 0 });
  return [
    { label: 'A host holds the processing lease', value: f.lease_held ? 'yes' : 'no', ok: f.lease_held },
    none('Jobs not stepped for 10 minutes', f.jobs_stuck),
    none('Calls whose transcription failed', f.calls_failed),
    none('Calls filed twice in the last 7 days', f.calls_filed_twice),
    none('Files left in a source for an hour', f.files_left_in_sources),
  ];
}

/** A call's transcription state in words. */
export const transcriptState = (state: string) =>
  ({ text: 'text', untried: 'not tried', no_speech: 'no speech' })[state] ?? state;

/** An estimate's number, or a dash when the planner has none. */
export const about = (n: number | null) => (n === null ? '—' : `about ${n.toLocaleString('en-US')}`);

/** A warning for each storage location a running host cannot use: its plays
 * of those calls fail there, and nothing is moved into it. An older server
 * sends no list. */
export const locationWarnings = (locations: AdminStatus['locations'] | undefined): string[] =>
  (locations ?? []).map(
    (l) => `${l.host} cannot use the storage location ${l.location}: ${l.problem ?? 'it cannot be read'}. Its calls there do not play from ${l.host}.`,
  );

/** A warning for each clock to look at: the machine's or recorder's name,
 * then what is wrong with its clock. */
export const clockWarnings = (clocks: ClockWarning[] | undefined): string[] =>
  (clocks ?? []).map((c) => `${c.subject}: ${c.text}.`);

/** A clock's warning as a sentence of its own, under its machine or recorder. */
export const clockSentence = (text: string): string => `${text.charAt(0).toUpperCase()}${text.slice(1)}.`;

/** The warning when the referee does not answer; none when it does or there is none. */
export function refereeWarning(referee: AdminStatus['referee']): { text: string; reason: string | undefined } | null {
  if (referee == null || referee.answers) return null;
  return { text: 'Referee unreachable: automatic failover is off until it returns', reason: referee.error };
}
