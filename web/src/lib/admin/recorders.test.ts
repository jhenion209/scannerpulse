import { describe, expect, it } from 'vitest';
import {
  changedText,
  changes,
  folderProblems,
  inFolder,
  lastText,
  nameDecidedText,
  previewText,
  seenText,
  stateText,
  systemDecidedText,
  todayText,
  verdictText,
  type Counts,
  type Preview,
  type Recorder,
} from './recorders';

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
  staged_today: 0,
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
    expect(todayText(recorder({ staged_today: 3 }))).toBe('9,881 filed, 2 rejected, 3 staged');
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

describe('a preview', () => {
  const zero: Counts = { filed: 0, new_talkgroup: 0, named: 0, proposal: 0, staged: 0, rejected: 0, unreadable: 0 };
  const preview = (counts: Partial<Counts>, files: number, read: number): Preview => ({
    recorder: 'east',
    files,
    counts: { ...zero, ...counts },
    sample: Array.from({ length: read }, (_, n) => ({
      rel_path: `call-${n}.mp3`,
      started_at: null,
      system: null,
      site: null,
      talkgroup: null,
      talkgroup_name: null,
      radio_id: null,
      radio_alias: null,
      duration_ms: null,
      verdict: { kind: 'filed' },
    })),
  });

  it('counts what filing would do first, the files filed with what they bring', () => {
    expect(previewText(preview({ filed: 2, new_talkgroup: 1, named: 1, proposal: 1, staged: 1, rejected: 1 }, 7, 7))).toBe(
      '7 files in its folder, all read: 5 filed (1 under a new talkgroup, 1 naming a placeholder, 1 with a name to approve), 1 staged, 1 rejected.',
    );
    expect(previewText(preview({ filed: 50 }, 1240, 50))).toBe('1,240 files in its folder, 50 read, spread over it: 50 filed.');
    expect(previewText(preview({ unreadable: 1 }, 1, 1))).toBe('1 file in its folder, all read: 1 cannot be read now.');
    expect(previewText(preview({}, 0, 0))).toBe('Its folder holds no audio file.');
  });

  it('says what filing would do with each file', () => {
    expect(verdictText({ kind: 'filed' })).toBe('Filed');
    expect(verdictText({ kind: 'new_talkgroup' })).toBe('Filed under a new talkgroup');
    expect(verdictText({ kind: 'named', current: 'Unidentified 8' })).toBe('Filed; names the talkgroup now called Unidentified 8');
    expect(verdictText({ kind: 'proposal', current: 'Group 7' })).toBe('Filed under Group 7; its name waits for your approval');
    expect(verdictText({ kind: 'staged' })).toBe('Staged: its system is unknown');
    expect(verdictText({ kind: 'rejected', reason: 'the file holds no MPEG audio frame' })).toBe(
      'Rejected: the file holds no MPEG audio frame',
    );
    expect(verdictText({ kind: 'unreadable', reason: 'Permission denied' })).toBe('Cannot be read now: Permission denied');
  });
});

describe('what waits for you', () => {
  it('says what each decision did', () => {
    expect(systemDecidedText('County-P25', 'new-system')).toBe(
      'County-P25 is a new system: its staged files are filed within a few seconds.',
    );
    expect(systemDecidedText('County-P25', 'map', { to: 'County P25' })).toBe(
      'County-P25 is County P25 from now on: its staged files are filed under it within a few seconds, and wait for a correction on the Data area.',
    );
    expect(systemDecidedText('County-P25', 'discard', { discarded: 412 })).toBe('412 staged files of County-P25 are deleted.');
    expect(systemDecidedText('County-P25', 'discard', { discarded: 1 })).toBe('1 staged file of County-P25 is deleted.');
    const p = { system: 'County P25', number: 1003, current: 'Fire Tac 3', proposed: 'Fire Tac 3 (Interop)' };
    expect(nameDecidedText(p, 'approve')).toBe(
      'County P25 1003 is Fire Tac 3 (Interop); its files keep the old name until they are corrected on the Data area.',
    );
    expect(nameDecidedText(p, 'dismiss')).toBe(
      'Fire Tac 3 (Interop) is dismissed for County P25 1003: that spelling is ignored from now on.',
    );
  });

  it('says when a name was first and last seen, in the site time', () => {
    expect(seenText('2031-01-14T20:02:00Z', '2031-01-15T17:40:00Z', NOW, 'America/Chicago')).toBe(
      '2031-01-14 14:02:00 – 11:40:00',
    );
  });
});
