import { describe, expect, it } from 'vitest';
import { backupAge, certificateEnd, COMMANDS, kindName } from './machine';

const now = new Date('2026-10-18T12:00:00Z');

describe('a kind of backup', () => {
  it('is fine when its newest is from the last night or so', () => {
    expect(backupAge('2026-10-18T03:17:40Z', now)).toEqual({ text: '8 h 42 min ago', ok: true });
  });

  it('needs a look when a night was missed, or none was ever taken', () => {
    expect(backupAge('2026-10-16T03:17:00Z', now)).toEqual({ text: '2 d 8 h ago', ok: false });
    expect(backupAge(null, now)).toEqual({ text: 'none yet', ok: false });
  });

  it('has a name people know', () => {
    expect(kindName('pgbackrest')).toBe('pgBackRest');
    expect(kindName('dump')).toBe('Dump');
  });
});

describe("the certificate's end", () => {
  it('is fine far off, and flagged within 30 days or past', () => {
    expect(certificateEnd('2031-10-18T12:00:00Z', now).ok).toBe(true);
    expect(certificateEnd('2026-11-01T12:00:00Z', now)).toEqual({ text: 'ends in 14 d', ok: false });
    expect(certificateEnd('2026-10-17T12:00:00Z', now)).toEqual({ text: 'ended 1 d ago', ok: false });
  });
});

describe('the commands', () => {
  it('are run as root on the machine itself', () => {
    for (const command of [COMMANDS.certificate, COMMANDS.ownCertificate, COMMANDS.push('dump'), COMMANDS.location]) {
      expect(command.startsWith('sudo /usr/local/bin/scannerpulse ')).toBe(true);
    }
    expect(COMMANDS.push('dump')).toContain('backups push dump');
  });
});
