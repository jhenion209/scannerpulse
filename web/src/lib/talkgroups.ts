import type { TalkgroupGroup } from './api';

/** The groups whose talkgroups match `text` (name or number), each with only
 * its matching talkgroups; a system with none is left out. */
export function matchGroups(groups: TalkgroupGroup[], text: string): TalkgroupGroup[] {
  const needle = text.trim().toLowerCase();
  if (!needle) return groups;
  return groups
    .map((group) => ({
      ...group,
      talkgroups: group.talkgroups.filter(
        (t) => t.name.toLowerCase().includes(needle) || String(t.number).startsWith(needle),
      ),
    }))
    .filter((group) => group.talkgroups.length > 0);
}

/** Adds or removes a talkgroup from the chosen ones. */
export function toggle(chosen: number[], id: number): number[] {
  return chosen.includes(id) ? chosen.filter((t) => t !== id) : [...chosen, id];
}
