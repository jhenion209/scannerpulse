import { describe, expect, it } from 'vitest';
import { parseValue, showValue, withPrefix, type Setting } from './settings';

describe('a typed value', () => {
  it('is JSON when it reads as JSON', () => {
    expect(parseValue('true')).toEqual({ value: true });
    expect(parseValue(' 30 ')).toEqual({ value: 30 });
    expect(parseValue('["Valley", "Metro"]')).toEqual({ value: ['Valley', 'Metro'] });
    expect(parseValue('"10"')).toEqual({ value: '10' });
  });

  it('is text when it does not, and nothing when empty', () => {
    expect(parseValue('Europe/Berlin')).toEqual({ value: 'Europe/Berlin' });
    expect(parseValue('  /srv/holding ')).toEqual({ value: '/srv/holding' });
    expect(parseValue('   ')).toBeNull();
  });
});

describe('a stored value in its field', () => {
  it('shows text bare, unless bare it would read as something else', () => {
    expect(showValue('Europe/Berlin')).toBe('Europe/Berlin');
    expect(showValue('true')).toBe('"true"');
    expect(showValue('10')).toBe('"10"');
    expect(showValue('')).toBe('""');
    expect(showValue(' x ')).toBe('" x "');
    expect(showValue('a\nb')).toBe('"a\\nb"');
    expect(showValue(false)).toBe('false');
    expect(showValue(['a', 'b'])).toBe('["a","b"]');
  });

  it('comes back as the same value when saved unchanged', () => {
    for (const value of ['Europe/Berlin', 'true', '10', '', ' x ', 'a\nb', 7, true, ['x'], { url: 'http://h/' }]) {
      expect(parseValue(showValue(value))).toEqual({ value });
    }
  });
});

describe('a group of settings', () => {
  it('is the settings under one prefix', () => {
    const rows: Setting[] = ['oidc.issuer', 'oidc.client_id', 'time_zone'].map((key) => ({ key, value: null, updated_at: null, known: true }));
    expect(withPrefix(rows, 'oidc.').map((s) => s.key)).toEqual(['oidc.issuer', 'oidc.client_id']);
  });
});
