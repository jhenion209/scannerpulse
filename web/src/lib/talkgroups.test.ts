import { describe, expect, it } from 'vitest';
import { matchGroups, toggle } from './talkgroups';

const groups = [
  { system: 'SYSA', system_id: 1, talkgroups: [{ id: 1, number: 101, name: 'Fire Dispatch' }, { id: 2, number: 205, name: 'Law 1' }] },
  { system: 'SYSB', system_id: 2, talkgroups: [{ id: 3, number: 1010, name: 'Public Works' }] },
];

describe('the talkgroup picker', () => {
  it('keeps every group when nothing is typed', () => {
    expect(matchGroups(groups, '  ')).toBe(groups);
  });

  it('matches names anywhere and numbers from the start, keeping the system', () => {
    expect(matchGroups(groups, 'fire').map((g) => g.talkgroups.map((t) => t.id))).toEqual([[1]]);
    expect(matchGroups(groups, '10').map((g) => g.system)).toEqual(['SYSA', 'SYSB']);
    expect(matchGroups(groups, '05')).toEqual([]);
  });

  it('chooses and unchooses a talkgroup', () => {
    expect(toggle([1, 2], 2)).toEqual([1]);
    expect(toggle([1], 3)).toEqual([1, 3]);
  });
});
