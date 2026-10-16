import { describe, expect, it } from 'vitest';
import { MAX_TEXT, sharedCallId, shareUrl, textProblem } from './actions';

describe('editing a call\'s text', () => {
  it('accepts plain text with new lines, up to 4,000 characters', () => {
    expect(MAX_TEXT).toBe(4000);
    expect(textProblem('Engine 7 responding.\nSecond line.')).toBeNull();
    expect(textProblem('é'.repeat(4000))).toBeNull();
  });

  it('refuses what the server refuses, in the server\'s words', () => {
    expect(textProblem('x'.repeat(4001))).toBe('a transcript has at most 4000 characters');
    expect(textProblem('a <b> tag')).toBe('a transcript cannot contain < or >');
    expect(textProblem('tab\there')).toBe('a transcript cannot contain control characters other than a new line');
  });
});

describe('sharing a call', () => {
  it('links to the call\'s own page', () => {
    expect(shareUrl('https://scanner.example.org', 31015)).toBe('https://scanner.example.org/calls/31015');
  });

  it('reads the call a page\'s path names, and nothing else', () => {
    expect(sharedCallId('/calls/31015')).toBe(31015);
    expect(sharedCallId('/calls/31015/')).toBe(31015);
    expect(sharedCallId('/')).toBeNull();
    expect(sharedCallId('/calls/abc')).toBeNull();
    expect(sharedCallId('/calls/99999999999999999999')).toBeNull();
    expect(sharedCallId('/admin/calls/7')).toBeNull();
  });
});
