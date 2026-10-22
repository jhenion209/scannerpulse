// The long jobs (`/api/admin/jobs`) as the Jobs area shows them: what each
// is, how far it has come, why it waits, and whether it can be stopped.

export interface Job {
  id: number;
  kind: string;
  params: Record<string, unknown>;
  state: 'queued' | 'running' | 'done' | 'failed' | 'stopped';
  done: number;
  total: number | null;
  message: string;
  started_by: string;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  updated_at: string | null;
  host: string | null;
  stop_requested: boolean;
}

/** How often the jobs are read again, for the list and the side menu's badge. */
export const JOBS_EVERY_MS = 10_000;

const KINDS: Record<string, string> = {
  queue: 'Queue past calls for transcription',
  asr_run: "A candidate's transcription run",
  correct: "Correct a talkgroup's files",
  merge: 'Merge two talkgroups',
  purge: 'Purge a talkgroup',
  move: 'Move a date range',
  purge_before: 'Purge older than a day',
};

export const kindName = (kind: string) => KINDS[kind] ?? kind;

/** A job that has not ended. */
export const underWay = (job: Job) => job.state === 'queued' || job.state === 'running';

/** How many jobs have not ended: the side menu's badge. */
export const activeCount = (jobs: Job[]) => jobs.filter(underWay).length;

/** The job's state in words, with a stop that is still to take effect. */
export function stateText(job: Job): string {
  if (job.state === 'running' && job.stop_requested) return 'stopping after its current step';
  return job.state;
}

/** `done of total`, or `done` when the total is not known. */
export const progressText = (job: Job) =>
  job.total === null ? job.done.toLocaleString('en-US') : `${job.done.toLocaleString('en-US')} of ${job.total.toLocaleString('en-US')}`;

/** The share done, 0 to 100; null when the total is not known. */
export const percent = (job: Job) =>
  job.total === null || job.total <= 0 ? null : Math.min(100, Math.round((job.done / job.total) * 100));

/** Why a step waits (its message is `waiting: …`), or null. */
export const waitingFor = (job: Job) => (job.message.startsWith('waiting: ') ? job.message.slice('waiting: '.length) : null);

/** A job can be asked to stop until it ends, once. */
export const canStop = (job: Job) => underWay(job) && !job.stop_requested;
