import { describe, expect, it } from 'vitest';
import { planText, progressText, stateText, type GuideStep } from './convert';

const step = (id: string, state: GuideStep['state']): GuideStep => ({
  id,
  title: `Step ${id}`,
  text: '',
  on: null,
  run: [],
  ask: null,
  state,
});

describe("the conversion's guide in words", () => {
  it('names the step to take now, and its place', () => {
    const guide = { steps: [step('a', 'done'), step('b', 'now'), step('c', 'later')], checks: [], complete: false, back: null };
    expect(progressText(guide)).toBe('Step 2 of 3: Step b.');
    expect(stateText('now')).toBe('now');
  });

  it('says when the conversion is complete', () => {
    const guide = { steps: [step('a', 'done')], checks: [{ text: 'x', ok: true }], complete: true, back: null };
    expect(progressText(guide)).toContain('The conversion is complete');
  });

  it('tells how to begin', () => {
    expect(planText).toContain('sudo /usr/local/bin/scannerpulse setup');
  });
});
