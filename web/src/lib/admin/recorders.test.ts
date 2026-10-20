import { describe, expect, it } from 'vitest';
import { changedText, changes, folderProblems, inFolder, lastText, stateText, todayText, type Recorder } from './recorders';

const NOW = new Date('2031-01-15T18:00:00Z');

const recorder = (over: Partial<Recorder> = {}): Recorder => ({
  id: 2,
  name: 'east',
  folder: '/srv/recorders/east',
  state: 'watched',
  bringing_in: false,
  last_file: '2031-01-15T17:59:48Z',
  waiting: 0,
  filed_today: 9881,
  rejected_today: 2,
  hosts: [{ host: 'host-a', ok: true, problem: null, files: 3, checked_at: '2031-01-15T17:59:58Z' }],
  ...over,
});

describe('a recorder card', () => {
  it('names how its files are taken in, and offers the changes that apply', () => {
    expect([stateText(recorder()), changes(recorder())]).toEqual(['Watched', ['pause', 'ask', 'remove']]);
    const paused = recorder({ state: 'paused' });
    expect([stateText(paused), changes(paused)]).toEqual(['Paused', ['watch', 'ask', 'remove']]);
    const asked = recorder({ state: 'asked' });
    expect([stateText(asked), changes(asked)]).toEqual(['When asked', ['bring-in', 'watch', 'remove']]);
    expect(stateText(recorder({ state: 'asked', bringing_in: true }))).toBe('Bringing in');
  });

  it("says its day's counts and when its last file came", () => {
    expect(todayText(recorder())).toBe('9,881 filed, 2 rejected');
    expect(todayText(recorder({ waiting: 1240 }))).toBe('9,881 filed, 2 rejected, 1,240 waiting');
    expect(lastText(recorder(), NOW)).toBe('12 s ago');
    expect(lastText(recorder({ last_file: null }), NOW)).toBe('never');
  });

  it('counts the files a recorder taken in when asked holds, by the newest count', () => {
    const asked = recorder({
      state: 'asked',
      hosts: [
        { host: 'host-a', ok: true, problem: null, files: 1240, checked_at: '2031-01-15T17:59:00Z' },
        { host: 'host-b', ok: true, problem: null, files: 1241, checked_at: '2031-01-15T17:59:30Z' },
        { host: 'host-c', ok: false, problem: 'cannot be listed', files: null, checked_at: '2031-01-15T17:59:59Z' },
      ],
    });
    expect(inFolder(asked)).toBe('1,241 files in its folder');
    expect(inFolder(recorder({ hosts: [] }))).toBeNull();
  });

  it('names each running host that cannot read the folder, has not listed it, or stopped', () => {
    const r = recorder({
      hosts: [
        { host: 'host-a', ok: true, problem: null, files: 3, checked_at: '2031-01-15T17:59:58Z' },
        { host: 'host-b', ok: false, problem: 'cannot be listed: No such file or directory', files: null, checked_at: '2031-01-15T17:59:58Z' },
        { host: 'host-c', ok: true, problem: null, files: 3, checked_at: '2031-01-15T17:00:00Z' },
      ],
    });
    expect(folderProblems(r, ['host-a', 'host-b', 'host-c', 'host-d'], NOW)).toEqual([
      'host-b cannot read it: cannot be listed: No such file or directory.',
      'host-c has not listed it for 1 h.',
      'host-d has not listed the folder yet.',
    ]);
    expect(folderProblems(recorder({ state: 'paused', hosts: [] }), ['host-a'], NOW)).toEqual([]);
  });

  it('says what each change did', () => {
    expect(changedText('east', 'bring-in')).toBe(
      "east's files are brought in: those its folder holds now are filed within a few seconds.",
    );
    expect(changedText('east', 'remove')).toBe('east is removed: its files stay in its folder, its calls in the archive.');
  });
});
