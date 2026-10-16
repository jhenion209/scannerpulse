// The one line of news a page shows for a few seconds: a file saved, a link
// copied, a refusal.

export const notice = $state({ text: null as string | null });

let timer: ReturnType<typeof setTimeout> | undefined;

/** Shows `text` for six seconds, in place of any line shown. */
export function tell(text: string) {
  notice.text = text;
  clearTimeout(timer);
  timer = setTimeout(() => (notice.text = null), 6000);
}
