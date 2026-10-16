import { describe, expect, it } from 'vitest';
import { talkgroupNames } from './favorites';

const groups = [
  { system: 'Metro', system_id: 1, talkgroups: [{ id: 1, number: 101, name: 'Fire' }, { id: 2, number: 102, name: 'Police' }] },
  { system: 'Valley', system_id: 2, talkgroups: [{ id: 3, number: 201, name: 'Ambulance' }] },
];

describe("a list's talkgroups", () => {
  it('are named with their system, in the groups order', () => {
    expect(talkgroupNames([3, 1], groups)).toEqual(['Fire · Metro', 'Ambulance · Valley']);
  });

  it('show a talkgroup no group knows by its id', () => {
    expect(talkgroupNames([2, 99], groups)).toEqual(['Police · Metro', 'talkgroup #99']);
    expect(talkgroupNames([], groups)).toEqual([]);
  });
});
