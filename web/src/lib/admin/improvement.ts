// The transcription improvement module's figures, as /api/admin/asr/* give them.

/** One set of sample calls. */
export interface AsrSet {
  set: string;
  size: number;
  /** Its calls that have a person's text. */
  labeled: number;
  runs: number;
}

/** A score against the people's texts. */
export interface AsrScore {
  wer: number | null;
  errors: number;
  words: number;
  calls: number;
  silent: number;
  phantom: number;
  summary: string;
}

/** A kept run of a candidate. */
export interface AsrRun {
  run_at: string;
  server: string;
  model: string;
  prompt: string | null;
  calls: number;
  wer: number;
  seconds_per_call: number;
}

export interface AsrReport {
  set: string;
  size: number;
  labeled: number;
  production: AsrScore;
  runs: AsrRun[];
}

/** A word error rate as the command line gives it: `12.3%`. */
export const werText = (wer: number | null) => (wer === null ? 'not known' : `${(wer * 100).toFixed(1)}%`);
