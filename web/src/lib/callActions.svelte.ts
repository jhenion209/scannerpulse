// The actions on one call that need no dialog of their own: download it
// under its file's name, and copy a link to it.

import { shareUrl } from './actions';
import { fetchCall, type Call } from './api';
import { tell } from './notice.svelte';

/** The link the share dialog shows when the browser will not copy it. */
export const sharing = $state({ url: null as string | null });

/** Downloads the call under its file's own name. */
export async function download(call: Call) {
  try {
    const detail = await fetchCall(call.id);
    const link = document.createElement('a');
    link.href = `/audio/${call.id}`;
    link.download = detail.file;
    document.body.append(link);
    link.click();
    link.remove();
  } catch (error) {
    tell(`The call could not be downloaded: ${error instanceof Error ? error.message : String(error)}`);
  }
}

/** Copies a link to the call; where the browser will not, shows it to copy by hand. */
export async function share(call: Call) {
  const url = shareUrl(location.origin, call.id);
  try {
    await navigator.clipboard.writeText(url);
    tell(`Link copied: ${url}`);
  } catch {
    sharing.url = url;
  }
}
