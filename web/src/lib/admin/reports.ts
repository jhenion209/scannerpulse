// The traffic report, as /api/admin/reports/traffic gives it.

/** One of the quietest hours: its local start and end, `03:25`. */
export interface Quiet {
  start: string;
  end: string;
  calls_per_day: number;
}

export interface Traffic {
  days: number;
  from: string;
  until: string;
  time_zone: string;
  weekdays: number;
  weekend_days: number;
  /** Calls a day in each five-minute slot, from local midnight: 288. */
  weekday: number[];
  weekend: number[];
  quietest_weekday: Quiet[];
  quietest_weekend: Quiet[];
  calls: number;
  transcribed: number;
  no_speech: number;
}

/** Calls a day in each of the day's 24 hours, from its five-minute slots. */
export const hourly = (slots: number[]) =>
  Array.from({ length: 24 }, (_, hour) => slots.slice(hour * 12, hour * 12 + 12).reduce((sum, v) => sum + v, 0));
