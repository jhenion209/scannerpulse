// The guide through an install's conversion to high availability
// (`/api/admin/convert`): the steps from the plan setup's planner saved, the
// one to take now, and the checks of the last.

import type { HelperKind } from './updates';

export interface ConvertPlan {
  nodes: [string, string];
  hub: string;
  referee: string | null;
  second: string | null;
  shared: string | null;
  archive: string | null;
}

export interface GuideStep {
  id: string;
  title: string;
  /** What it does, and how it is checked. */
  text: string;
  /** Where the person runs `run`. */
  on: string | null;
  run: string[];
  /** What the root helper does once an admin approves it. */
  ask: { kind: HelperKind; label: string; question: string } | null;
  state: 'done' | 'now' | 'later';
}

export interface ConvertInfo {
  /** Null until setup's planner has saved a plan. */
  plan: ConvertPlan | null;
  guide?: { steps: GuideStep[]; checks: { text: string; ok: boolean }[]; complete: boolean };
}

/** How to begin, on one machine with no plan yet. */
export const planText =
  'To convert this install to high availability, run sudo /usr/local/bin/scannerpulse setup on this machine: it plans the conversion, and this area then guides each step.';

/** Where the conversion stands. */
export function progressText(guide: NonNullable<ConvertInfo['guide']>): string {
  if (guide.complete)
    return 'The conversion is complete: every step is done and every check passes. A test switchover is under Switchover, below.';
  const now = guide.steps.findIndex((step) => step.state === 'now');
  return `Step ${now + 1} of ${guide.steps.length}: ${guide.steps[now].title}.`;
}

/** A step's state in a word. */
export const stateText = (state: GuideStep['state']) =>
  ({ done: 'done', now: 'now', later: 'later' })[state];
