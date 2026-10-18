import { describe, expect, it } from 'vitest';
import { backupAge, certificateEnd, COMMANDS, kindName, proofName, renewalText } from './machine';

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

describe("Let's Encrypt's renewals", () => {
  const acme = { proof: 'web' as const, names: ['radio.example.org'], tried: null, renewed: null, error: null };

  it('say how the names are proved', () => {
    expect(proofName('web')).toBe('over the web');
    expect(proofName('dns-cloudflare')).toBe('through DNS at Cloudflare');
  });

  it('say when the certificate came, or that a request is under way', () => {
    expect(renewalText(acme, now)).toEqual({ text: 'renewed when a third of its life is left', ok: true });
    expect(renewalText({ ...acme, tried: '2026-10-18T11:59:00Z' }, now)).toEqual({
      text: 'asked 1 min ago, no answer yet',
      ok: true,
    });
    expect(
      renewalText({ ...acme, tried: '2026-10-18T09:00:00Z', renewed: '2026-10-18T09:01:00Z' }, now),
    ).toEqual({ text: 'renewed 2 h 59 min ago', ok: true });
  });

  it("flag the authority's refusal in its own words", () => {
    expect(
      renewalText({ ...acme, tried: '2026-10-18T11:00:00Z', error: 'radio.example.org: Connection refused' }, now),
    ).toEqual({
      text: 'the request 1 h ago failed: radio.example.org: Connection refused (asked again within the hour)',
      ok: false,
    });
  });
});

describe('the commands', () => {
  it('are run as root on the machine itself', () => {
    for (const command of [
      COMMANDS.certificate,
      COMMANDS.ownCertificate,
      COMMANDS.letsEncryptWeb,
      COMMANDS.letsEncryptDns,
      COMMANDS.push('dump'),
      COMMANDS.location,
    ]) {
      expect(command.startsWith('sudo /usr/local/bin/scannerpulse ')).toBe(true);
    }
    expect(COMMANDS.push('dump')).toContain('backups push dump');
    expect(COMMANDS.letsEncryptDns).toContain('tls letsencrypt dns-cloudflare');
    expect(COMMANDS.letsEncryptDns).toContain('asks for the Cloudflare token');
  });
});
