import { describe, expect, it } from 'vitest';
import { takenBy } from './data';
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
