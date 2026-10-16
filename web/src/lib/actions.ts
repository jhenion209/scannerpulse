// The actions on one call: correcting its text, downloading it and sharing a
// link to it.

/** The longest text the server keeps. */
export const MAX_TEXT = 4000;

/** Why the server would refuse this text, in its own words; null when it would not. */
export function textProblem(text: string): string | null {
  if ([...text].length > MAX_TEXT) return `a transcript has at most ${MAX_TEXT} characters`;
  if (/[<>]/.test(text)) return 'a transcript cannot contain < or >';
  // Control characters other than a new line (the server's rule).
  if ([...text].some((c) => c !== '\n' && /\p{Cc}/u.test(c))) {
    return 'a transcript cannot contain control characters other than a new line';
  }
  return null;
}

/** The address of a call's own page. */
export const shareUrl = (origin: string, id: number) => `${origin}/calls/${id}`;

/** The call a page's path names (`/calls/<id>`); null for any other path. */
export function sharedCallId(pathname: string): number | null {
  const match = /^\/calls\/(\d+)\/?$/.exec(pathname);
  if (!match) return null;
  const id = Number(match[1]);
  return Number.isSafeInteger(id) ? id : null;
}
