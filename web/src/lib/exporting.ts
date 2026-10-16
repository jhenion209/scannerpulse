// Export: the files the page makes itself (CSV, and the name of a printed
// PDF) and the requests for the ones the server builds (Excel, call audio).
// Every file follows the server's naming: the time range in the site's zone,
// then "ScannerPulse", then the talkgroups.

import type { Call } from './api';
import { COLUMNS, extraText, isExtra, type ColumnId } from './columns';
import { zoned } from './format';

/** What a file's name needs of a call. */
export interface NamePart {
  started_at: string;
  talkgroup: number | null;
  system: string | null;
}

/** The most talkgroups a name lists, and its longest length in bytes (the server's values). */
const NAME_TALKGROUPS = 5;
const NAME_BYTES = 200;

const stamp = (at: Date, zone: string) => {
  const { date, time } = zoned(at, zone);
  return `${date.replaceAll('-', '')}_${time.replaceAll(':', '')}`;
};

/** `20310701_130000-20310701_143000_ScannerPulse_TG-7-SYSA,9-SYSB.csv`. `parts`
 * are oldest first; the list's own range wins over the calls' when it has one. */
export function exportName(parts: NamePart[], from: Date | null, until: Date | null, zone: string, ext: string): string {
  const first = from ?? (parts.length ? new Date(parts[0].started_at) : null);
  const last = until ?? (parts.length ? new Date(parts[parts.length - 1].started_at) : null);
  const talkgroups: string[] = [];
  for (const p of parts) {
    if (p.talkgroup === null || p.system === null) continue;
    const tg = `${p.talkgroup}-${p.system}`;
    if (!talkgroups.includes(tg)) talkgroups.push(tg);
  }
  const range = first && last ? `${stamp(first, zone)}-${stamp(last, zone)}_` : '';
  let tg = '';
  if (talkgroups.length) {
    tg = `_TG-${talkgroups.slice(0, NAME_TALKGROUPS).join(',')}`;
    if (talkgroups.length > NAME_TALKGROUPS) tg += `,+${talkgroups.length - NAME_TALKGROUPS}`;
  }
  const bytes = (text: string) => new TextEncoder().encode(text).length;
  let stem = [...`${range}ScannerPulse${tg}`];
  while (bytes(stem.join('')) > NAME_BYTES - ext.length - 1) stem.pop();
  return `${stem.join('')}.${ext}`;
}

/** A CSV a spreadsheet opens as UTF-8; no cell can start a formula. */
export function csvText(rows: string[][]): string {
  const cell = (value: string) => {
    const safe = /^[=+\-@\t\r]/.test(value) ? `'${value}` : value;
    return /[",\r\n]/.test(safe) ? `"${safe.replaceAll('"', '""')}"` : safe;
  };
  return `\uFEFF${rows.map((row) => row.map(cell).join(',')).join('\r\n')}\r\n`;
}

/** The page's rows as a table: the columns on screen, the time split into
 * the date and the time in the site's zone. */
export function pageTable(calls: Call[], columns: ColumnId[], zone: string): string[][] {
  const shown = COLUMNS.filter((c) => columns.includes(c.id));
  const header = shown.flatMap((c) => (c.id === 'time' ? ['Date', 'Time'] : c.id === 'length' ? ['Length (s)'] : [c.label]));
  const value = (call: Call, id: ColumnId): string[] => {
    if (id === 'time') {
      const { date, time } = zoned(new Date(call.started_at), zone);
      return [date, time];
    }
    if (id === 'talkgroup') return [call.talkgroup_name ?? (call.talkgroup === null ? '' : String(call.talkgroup))];
    if (id === 'length') return [call.duration_ms === null ? '' : (call.duration_ms / 1000).toFixed(1)];
    if (id === 'text') return [call.transcript ?? ''];
    return isExtra(id) ? [extraText(call, id)] : [''];
  };
  return [header, ...calls.map((call) => shown.flatMap((c) => value(call, c.id)))];
}

/** The list's query for every match: without its page, count or SmartSort. */
export function exportQuery(list: URLSearchParams): URLSearchParams {
  const query = new URLSearchParams(list);
  for (const key of ['size', 'after', 'count', 'smart', 'exact']) query.delete(key);
  return query;
}

/** The audio export's request: the list's filter, and the chosen calls when there are some. */
export function audioBody(list: URLSearchParams, ids?: number[]): Record<string, unknown> {
  const body: Record<string, unknown> = {};
  for (const [key, value] of exportQuery(list)) body[key] = key === 'system' ? Number(value) : value;
  if (ids && ids.length) body.ids = ids;
  return body;
}

/** The file name a download's answer gives, the UTF-8 one first. */
export function nameFromHeader(header: string | null, fallback: string): string {
  if (!header) return fallback;
  const utf8 = /filename\*=UTF-8''([^;]+)/i.exec(header);
  if (utf8) {
    try {
      return decodeURIComponent(utf8[1]);
    } catch {
      // A malformed name: the plain one below, or the fallback.
    }
  }
  const plain = /filename="([^"]*)"/i.exec(header);
  return plain ? plain[1] : fallback;
}

/** Saves a file the page holds under `name`. */
export function saveFile(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
