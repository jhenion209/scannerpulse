import { describe, expect, it } from 'vitest';
import { matchTalkgroups, nextSort, previewText, queueBody, queueQuery, retriedText, sortTalkgroups, type AdminTalkgroup } from './transcription';

const tg = (id: number, number: number, name: string, system = 'Metro'): AdminTalkgroup => ({
  id, system, number, name, transcribe: false, files_correct: true,
});
const list = [tg(1, 101, 'Fire Dispatch'), tg(2, 102, 'Police One'), tg(3, 201, 'Ambulance', 'Valley')];

describe('finding a talkgroup', () => {
  it('matches a name, the start of a number, or a whole system', () => {
    expect(matchTalkgroups(list, 'fire').map((t) => t.id)).toEqual([1]);
    expect(matchTalkgroups(list, '10').map((t) => t.id)).toEqual([1, 2]);
    expect(matchTalkgroups(list, 'valley').map((t) => t.id)).toEqual([3]);
    expect(matchTalkgroups(list, '  ')).toHaveLength(3);
  });
});

describe('sorting the talkgroups', () => {
  const on = (t: AdminTalkgroup) => ({ ...t, transcribe: true });
  const many = [tg(1, 145, 'fire'), tg(2, 9, 'Police'), on(tg(3, 9, 'ambulance', 'Valley')), on(tg(4, 20, 'Water'))];
  const ids = (sorted: AdminTalkgroup[]) => sorted.map((t) => t.id);

  it('sorts numbers by value, not as text', () => {
    expect(ids(sortTalkgroups(many, { column: 'number', descending: false }))).toEqual([2, 3, 4, 1]);
    expect(ids(sortTalkgroups(many, { column: 'number', descending: true }))).toEqual([1, 4, 2, 3]);
  });

  it('sorts names and systems without regard to case', () => {
    expect(ids(sortTalkgroups(many, { column: 'name', descending: false }))).toEqual([3, 1, 2, 4]);
    expect(ids(sortTalkgroups(many, { column: 'system', descending: true }))).toEqual([3, 2, 4, 1]);
  });

  it('puts the talkgroups that are on first, then by system and number', () => {
    expect(ids(sortTalkgroups(many, { column: 'transcribe', descending: false }))).toEqual([4, 3, 2, 1]);
    expect(ids(sortTalkgroups(many, { column: 'transcribe', descending: true }))).toEqual([2, 1, 4, 3]);
  });

  it('leaves the list it is given as it was', () => {
    const before = ids(many);
    sortTalkgroups(many, { column: 'name', descending: true });
    expect(ids(many)).toEqual(before);
  });

  it('flips the order on the same column and starts a new column ascending', () => {
    expect(nextSort({ column: 'name', descending: false }, 'name')).toEqual({ column: 'name', descending: true });
    expect(nextSort({ column: 'name', descending: true }, 'name')).toEqual({ column: 'name', descending: false });
    expect(nextSort({ column: 'name', descending: true }, 'number')).toEqual({ column: 'number', descending: false });
  });
});

describe('the queue job', () => {
  it('names a talkgroup and days only when they are given', () => {
    expect(queueBody({ talkgroup: null, from: '', to: '' })).toEqual({});
    expect(queueBody({ talkgroup: 4, from: '2031-07-01', to: '2031-07-02' })).toEqual({ talkgroup_id: 4, from: '2031-07-01', to: '2031-07-02' });
    expect(queueQuery({ talkgroup: 4, from: '2031-07-01', to: '' }).toString()).toBe('talkgroup_id=4&from=2031-07-01');
  });

  it('says what the preview found', () => {
    expect(previewText(1234, 1000)).toBe('1,234 calls would be queued (calls shorter than 1 s are left out).');
    expect(previewText(1, 0)).toBe('1 call would be queued.');
    expect(previewText(0, 1500)).toBe('No call to queue (calls shorter than 1.5 s are left out).');
  });
});

describe('the failed calls put back', () => {
  it('says how many, in the singular for one', () => {
    expect(retriedText(0)).toBe('No failed call to try again.');
    expect(retriedText(1)).toBe('1 failed call is back in the queue.');
    expect(retriedText(1234)).toBe('1,234 failed calls are back in the queue.');
  });
});
