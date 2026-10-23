import { describe, expect, it } from 'vitest';
import {
  callsText, consolidateAction, consolidateAsk, consolidateText, freeText, importAction, moveAction, moveAsk, offsetText,
  previewText, purgeAction, purgeAsk, purgeText, reviewLines, seenBy, sizeText,
  type ConsolidatePreview, type Location, type MovePreview, type PurgePreview, type Review,
} from './storage';

const now = new Date('2026-10-21T18:00:00Z');

const cold = (over: Partial<Location> = {}): Location => ({
  id: 2,
  name: 'Cold 2024',
  root: '/srv/cold-2024',
  mode: 'managed',
  primary: false,
  offset_minutes: null,
  hosts: [
    { host: 'host-a', ok: true, problem: null, free_bytes: 1_100_000_000_000, total_bytes: 2_000_000_000_000, checked_at: '2026-10-21T17:59:30Z' },
    { host: 'host-b', ok: false, problem: '/srv/cold-2024 has no .scannerpulse-location file: the disk or share is not mounted there', free_bytes: null, total_bytes: null, checked_at: '2026-10-21T17:59:40Z' },
  ],
  calls: 3_204_118,
  first_call: '2024-01-01T06:00:00Z',
  last_call: '2024-12-31T23:00:00Z',
  bytes_about: 900_000_000_000,
  ...over,
});

const preview = (over: Partial<MovePreview> = {}): MovePreview => ({
  to: 'Cold 2024',
  to_id: 2,
  from_day: '2025-01-01',
  to_day: '2025-06-30',
  calls: 412_355,
  held: [{ location: 'Main archive', calls: 412_355 }],
  bytes_about: 96_000_000_000,
  free_bytes: 1_100_000_000_000,
  refused: null,
  ...over,
});

describe('a size', () => {
  it('reads as the server writes it, in thousands', () => {
    expect(sizeText(512)).toBe('512 bytes');
    expect(sizeText(1_500_000)).toBe('1.5 MB');
    expect(sizeText(2_100_000_000_000)).toBe('2.1 TB');
  });
});

describe("a location's card", () => {
  it('gives its calls by day in the site zone, and the space they use', () => {
    expect(callsText(cold(), 'America/Chicago')).toBe(
      'Calls from 2024-01-01 to 2024-12-31 · 3,204,118 calls · about 900.0 GB used',
    );
    expect(callsText(cold({ calls: 0, first_call: null, last_call: null }), 'UTC')).toBe('No call');
  });

  it('gives the space left by a host that can use it', () => {
    expect(freeText(cold())).toBe('1.1 TB free');
    expect(freeText(cold({ hosts: [] }))).toBeNull();
  });

  it('says for each running host whether it can use it', () => {
    expect(seenBy(cold(), ['host-a', 'host-b', 'host-c'], now)).toEqual([
      { host: 'host-a', ok: true, text: 'host-a ✓' },
      {
        host: 'host-b',
        ok: false,
        text: 'host-b ✗ cannot use it: /srv/cold-2024 has no .scannerpulse-location file: the disk or share is not mounted there',
      },
      { host: 'host-c', ok: false, text: 'host-c ✗ has not checked it yet' },
    ]);
    const old = cold({ hosts: [{ ...cold().hosts[0], checked_at: '2026-10-21T17:40:00Z' }] });
    expect(seenBy(old, ['host-a'], now)[0].text).toBe('host-a ✗ has not checked it for 20 min');
  });
});

describe('a move', () => {
  it('says what the count found', () => {
    expect(previewText(preview())).toBe(
      '412,355 calls (about 96.0 GB) of 2025-01-01 to 2025-06-30 would move into Cold 2024: 412,355 in Main archive. Cold 2024 has 1.1 TB free.',
    );
  });

  it('names what its last button and its pop-up do', () => {
    expect(moveAction(preview())).toBe('Move 412,355 calls to Cold 2024');
    expect(moveAction(preview({ calls: 1 }))).toBe('Move 1 call to Cold 2024');
    expect(moveAsk(preview())).toEqual({
      title: 'Move 412,355 calls to Cold 2024?',
      text: 'Each file is copied and checked, its call then points at Cold 2024, and the old file is deleted. Moving them back is a new move.',
      action: 'Move 412,355 calls',
    });
  });
});

describe('a purge', () => {
  const p: PurgePreview = {
    before: '2025-03-02',
    calls: 402_117,
    held: [
      { location: 'Main archive', read_only: false, calls: 400_000, bytes_about: 90_000_000_000 },
      { location: 'Old', read_only: true, calls: 2_117, bytes_about: null },
    ],
    bytes_about: 90_000_000_000,
    refused: null,
  };

  it('says what the count found, and which files stay', () => {
    expect(purgeText(p)).toBe(
      '402,117 calls before 2025-03-02 (about 90.0 GB) would be deleted for good: 400,000 in Main archive, 2,117 in Old (read-only: only their records go).',
    );
  });

  it('names what its last button and its pop-up do', () => {
    expect(purgeAction(p)).toBe('Delete 402,117 calls');
    expect(purgeAsk(p)).toEqual({
      title: 'Delete 402,117 calls before 2025-03-02?',
      text: 'Their records go now, and their files are deleted for good; a read-only location keeps its files.',
      action: 'Delete 402,117 calls',
    });
  });
});

describe('an import', () => {
  const review = (over: Partial<Review> = {}): Review => ({
    name: 'Old',
    folder: '/srv/old',
    files: 4_003,
    taken: 0,
    no_date: 3,
    bad_names: 0,
    left: 4_000,
    read: 1_000,
    new: 600,
    copies: 400,
    unknown: 0,
    differ: 10,
    unreadable: 0,
    before_purge: 0,
    examples: { no_date: ['notes/readme.mp3'], new: [], copies: [], unknown: [], differ: [], unreadable: [] },
    copies_at: [0, 0, 400, 10, 0],
    new_at: [600, 600, 600, 590, 600],
    suggested: null,
    kept_offset: null,
    done: true,
    ...over,
  });

  it('writes an offset as the server reads it', () => {
    expect(offsetText(60)).toBe('+01:00');
    expect(offsetText(-30)).toBe('-00:30');
    expect(offsetText(0)).toBe('+00:00');
  });

  it('says what the review found, scaled to the files left when it read a sample', () => {
    expect(reviewLines(review())).toEqual([
      '4,003 mp3 files in /srv/old; 3 with no date and time in their names are skipped.',
      'Of the 4,000 left, 1,000 were read: about 2,400 new calls, about 1,600 copies of calls already in the archive (skipped), 0 of systems not named yet (they wait), 0 unreadable (skipped).',
      'The folder’s own copies of a call count here as new calls: the import keeps one of each.',
      'About 40 new calls name their talkgroup otherwise: the names wait for approval on the Recorders area.',
      'Copies at each offset: -02:00 0, -01:00 0, +00:00 400, +01:00 10, +02:00 0.',
    ]);
    expect(reviewLines(review({ left: 5, read: 5, new: 4, copies: 1, differ: 0, copies_at: [0, 0, 1, 0, 0], new_at: [4, 4, 4, 5, 4], suggested: 60 }))).toEqual([
      '4,003 mp3 files in /srv/old; 3 with no date and time in their names are skipped.',
      'Of the 5 left, all were read: 4 new calls, 1 copy of a call already in the archive (skipped), 0 of systems not named yet (they wait), 0 unreadable (skipped).',
      'The folder’s own copies of a call count here as new calls: the import keeps one of each.',
      'Copies at each offset: -02:00 0, -01:00 0, +00:00 1, +01:00 0, +02:00 0.',
      'The files are copies once their times are moved by +01:00: the older install’s clock or time zone was off.',
      'At +01:00: 5 new calls, 0 copies of calls already in the archive.',
    ]);
    expect(reviewLines(review({ taken: 900, kept_offset: -60 })).at(-1)).toBe('This import keeps its offset, -01:00.');
  });

  it('names what its last button does, at the offset in the field', () => {
    expect(importAction(review(), 'Old', '')).toBe('Import up to about 2,400 calls into Old');
    expect(importAction(review({ new_at: [600, 600, 600, 213, 600] }), 'Old', '+01:00')).toBe('Import up to about 852 calls into Old');
    // An offset the review did not try gives no number.
    expect(importAction(review(), 'Old', '+00:30')).toBe('Import into Old');
    // An import that keeps its offset was reviewed at it.
    expect(importAction(review({ kept_offset: 30 }), 'Old', '+00:30')).toBe('Import up to about 2,400 calls into Old');
    expect(importAction(review({ left: 4, read: 4, new: 1 }), 'Old', '')).toBe('Import up to 1 call into Old');
  });

  it('is consolidated once counted, its word typed, and confirmed', () => {
    const p: ConsolidatePreview = { name: 'Old', calls: 2_117, bytes_about: 300_000_000, free_bytes: 400_000_000_000, refused: null };
    expect(consolidateText(p)).toBe(
      '2,117 calls of Old (about 300.0 MB) would be copied into the main archive, which has 400.0 GB free. Old’s own files stay as they are.',
    );
    expect(consolidateAction(p)).toBe('Copy 2,117 calls into the main archive');
    expect(consolidateAsk(p)).toEqual({
      title: 'Copy 2,117 calls of Old into the main archive?',
      text: 'Each file is copied and checked, and its call then points at the copy. Old’s files stay as they are; once it holds no call, Old can be removed.',
      action: 'Copy 2,117 calls',
    });
  });
});
