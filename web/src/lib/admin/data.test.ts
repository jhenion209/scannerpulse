import { describe, expect, it } from 'vitest';
import { correctText, mergeText, purgeText, readOnlyText, takenBy, type Named } from './data';
import type { AdminTalkgroup } from './transcription';

const tg = (id: number, system: string, number: number, name: string): AdminTalkgroup => ({
  id, system, number, name, transcribe: false, files_correct: true,
});

describe('takenBy', () => {
  const list = [tg(1, 'Metro', 102, 'North'), tg(2, 'Metro', 101, 'North'), tg(3, 'Valley', 201, 'South'), tg(4, 'Metro', 103, 'South')];
  it("finds the same system's talkgroup with the name, the lowest number first", () => {
    expect(takenBy(list, list[3], 'North')?.id).toBe(2);
  });
  it('matches the name exactly and never the talkgroup itself or another system', () => {
    expect(takenBy(list, list[3], 'north')).toBeUndefined();
    expect(takenBy(list, list[3], 'South')).toBeUndefined();
    expect(takenBy(list, list[0], 'South')?.id).toBe(4);
  });
});

describe('the counted sentences', () => {
  const north: Named = { id: 1, system: 'Metro', number: 101, name: 'Metro Fire North' };
  const south: Named = { id: 2, system: 'Metro', number: 102, name: 'Metro Fire South' };

  it('say what a correction moves', () => {
    expect(correctText({ talkgroup: north, calls: 10, read_only: 2 }))
      .toBe("10 files of Metro Fire North (Metro/101) move to its name's folder; 2 in read-only locations stay.");
    expect(correctText({ talkgroup: north, calls: 1, read_only: 1 }))
      .toBe("1 file of Metro Fire North (Metro/101) moves to its name's folder; 1 in a read-only location stays.");
    expect(correctText({ talkgroup: north, calls: 1234, read_only: 0 }))
      .toBe("1,234 files of Metro Fire North (Metro/101) move to its name's folder.");
  });

  it('say what a merge moves', () => {
    expect(mergeText({ from: south, into: north, calls: 12 }))
      .toBe('12 calls of Metro Fire South (Metro/102) go to Metro Fire North (Metro/101); Metro Fire South goes.');
    expect(mergeText({ from: south, into: north, calls: 1 }))
      .toBe('1 call of Metro Fire South (Metro/102) goes to Metro Fire North (Metro/101); Metro Fire South goes.');
  });

  it('say what a purge deletes, and what it cannot move', () => {
    expect(purgeText({ talkgroup: north, calls: 5, read_only: 0, holding_path: '/srv/holding' }))
      .toBe('5 calls of Metro Fire North (Metro/101) are deleted; their files go to /srv/holding.');
    expect(purgeText({ talkgroup: north, calls: 1, read_only: 0, holding_path: '/srv/holding' }))
      .toBe('1 call of Metro Fire North (Metro/101) is deleted; its file goes to /srv/holding.');
    expect(readOnlyText(2)).toBe('2 of the calls are in read-only locations, whose files a purge cannot move.');
    expect(readOnlyText(1)).toBe('1 of the calls is in a read-only location, whose file a purge cannot move.');
  });
});
