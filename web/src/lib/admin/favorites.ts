// The favorite lists as the Favorites area edits them: a list's talkgroups
// by name, from the filter choices' groups.

import type { TalkgroupGroup } from '../api';

/** Each talkgroup's name and system, in the groups' order; an id no group
 * knows (a talkgroup removed since) shows as its number. */
export function talkgroupNames(ids: number[], groups: TalkgroupGroup[]): string[] {
  const names: string[] = [];
  const found = new Set<number>();
  for (const group of groups) {
    for (const t of group.talkgroups) {
      if (ids.includes(t.id)) {
        names.push(`${t.name} · ${group.system}`);
        found.add(t.id);
      }
    }
  }
  return [...names, ...ids.filter((id) => !found.has(id)).map((id) => `talkgroup #${id}`)];
}
