import { describe, expect, it } from 'vitest';
import {
  callsText, freeText, moveAction, moveAsk, previewText, purgeAction, purgeAsk, purgeText, seenBy, sizeText,
  type Location, type MovePreview, type PurgePreview,
} from './storage';

const now = new Date('2026-10-21T18:00:00Z');

const cold = (over: Partial<Location> = {}): Location => ({
  id: 2,
  name: 'Cold 2024',
  root: '/srv/cold-2024',
  mode: 'managed',
  primary: false,
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
