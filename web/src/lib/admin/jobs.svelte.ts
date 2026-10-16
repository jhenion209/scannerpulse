// The jobs list, read every 10 seconds while an admin is signed in: the Jobs
// area shows it and the side menu counts the jobs under way.

import { JOBS_EVERY_MS, type Job } from './jobs';
import { admin, why } from './session.svelte';

export const jobs = $state({
  list: null as Job[] | null,
  error: null as string | null,
});

/** Reads the list; a failed read keeps the last one, with the failure. A read
 * the timer makes (`poll`) does not count as the admin's use of the session. */
async function read(poll: boolean) {
  try {
    jobs.list = await admin<Job[]>('GET', '/api/admin/jobs', undefined, { poll });
    jobs.error = null;
  } catch (error) {
    jobs.error = why(error);
  }
}

/** The admin's own read: after a change, or a click on "Read now". */
export const readJobs = () => read(false);

/** Reads the jobs now and every 10 s (the timer's reads are polls); returns the stop. */
export function followJobs(): () => void {
  void read(false);
  const timer = setInterval(() => void read(true), JOBS_EVERY_MS);
  return () => clearInterval(timer);
}
