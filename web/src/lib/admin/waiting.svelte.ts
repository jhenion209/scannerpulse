// What waits for the administrator's decision (unknown system names and
// talkgroup name proposals), read every 15 seconds while an admin is signed
// in: the Recorders area lists it and the side menu counts it.

import { STATUS_EVERY_MS } from './health';
import type { Waiting } from './recorders';
import { admin, why } from './session.svelte';

export const waiting = $state({
  data: null as Waiting | null,
  error: null as string | null,
});

/** Reads the list; a failed read keeps the last one, with the failure. A read
 * the timer makes (`poll`) does not count as the admin's use of the session. */
async function read(poll: boolean) {
  try {
    waiting.data = await admin<Waiting>('GET', '/api/admin/names', undefined, { poll });
    waiting.error = null;
  } catch (error) {
    waiting.error = why(error);
  }
}

/** The admin's own read, after a decision. */
export const readWaiting = () => read(false);

/** Reads now and every 15 s (the timer's reads are polls); returns the stop. */
export function followWaiting(): () => void {
  void read(false);
  const timer = setInterval(() => void read(true), STATUS_EVERY_MS);
  return () => clearInterval(timer);
}
