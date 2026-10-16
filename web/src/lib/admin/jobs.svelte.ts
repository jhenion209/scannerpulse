// The jobs list, read every 10 seconds while an admin is signed in: the Jobs
// area shows it and the side menu counts the jobs under way.

import { JOBS_EVERY_MS, type Job } from './jobs';
import { admin, why } from './session.svelte';

export const jobs = $state({
  list: null as Job[] | null,
  error: null as string | null,
});

/** Reads the list; a failed read keeps the last one, with the failure. */
export async function readJobs() {
  try {
    jobs.list = await admin<Job[]>('GET', '/api/admin/jobs');
    jobs.error = null;
  } catch (error) {
    jobs.error = why(error);
  }
}

/** Reads the jobs now and every 10 s; returns the stop. */
export function followJobs(): () => void {
  void readJobs();
  const timer = setInterval(readJobs, JOBS_EVERY_MS);
  return () => clearInterval(timer);
}
