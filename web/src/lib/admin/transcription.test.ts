import { describe, expect, it } from 'vitest';
import { matchTalkgroups, previewText, queueBody, queueQuery, retriedText, type AdminTalkgroup } from './transcription';

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
