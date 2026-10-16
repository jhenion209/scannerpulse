import { fetchStatus, type Status } from './api';
import { STATUS_REFRESH_MS, newer } from './status';

/** The last status read, the server's clock against this one, and a clock
 * that ticks every second so the chips' ages and time move on between reads. */
export const status = $state({
  data: null as Status | null,
  /** The server's clock minus this browser's, in ms. */
  offset: 0,
  now: Date.now(),
  error: null as string | null,
});

async function refresh() {
  try {
    const data = await fetchStatus();
    // A call heard live may be newer than the read: keep the later.
    if (status.data?.newest_call) data.newest_call = newer(data.newest_call, status.data.newest_call);
    status.data = data;
    status.offset = Date.parse(data.server_time) - Date.now();
    status.error = null;
  } catch (error) {
    status.error = error instanceof Error ? error.message : String(error);
  }
}

/** Reads the status now and every 30 s, and ticks the clock; returns the stop. */
export function startStatus(): () => void {
  void refresh();
  const reads = setInterval(refresh, STATUS_REFRESH_MS);
  const ticks = setInterval(() => (status.now = Date.now()), 1000);
  return () => {
    clearInterval(reads);
    clearInterval(ticks);
  };
}

/** A call heard on the live feed is the newest one at once. */
export function noteCall(startedAt: string) {
  if (status.data) status.data.newest_call = newer(status.data.newest_call, startedAt);
}

/** The server's time now. */
export const serverNow = () => new Date(status.now + status.offset);

/** The site's time zone; until the status is read, this browser's own. */
export const siteZone = () => status.data?.time_zone ?? Intl.DateTimeFormat().resolvedOptions().timeZone;
