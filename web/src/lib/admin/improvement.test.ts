import { describe, expect, it } from 'vitest';
import { werText } from './improvement';

describe('werText', () => {
  it('gives the rate as a percentage with one decimal', () => {
    expect(werText(0.1234)).toBe('12.3%');
    expect(werText(0)).toBe('0.0%');
  });
  it('says so when the rate is not known', () => {
    expect(werText(null)).toBe('not known');
  });
});
