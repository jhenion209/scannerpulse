import { describe, expect, it } from 'vitest';
import type { Call } from './api';
import { audioBody, csvText, exportName, exportQuery, nameFromHeader, pageTable } from './exporting';

const part = (iso: string, talkgroup: number | null, system: string | null) => ({ started_at: iso, talkgroup, system });

describe('the export file name (the server\'s convention, for the files the page makes)', () => {
  it('leads with the time range in the site\'s zone, then the talkgroups', () => {
    const parts = [
      part('2031-07-01T17:00:00Z', 7, 'SYSA'),
      part('2031-07-01T18:30:00Z', 9, 'SYSB'),
      part('2031-07-01T18:30:00Z', 7, 'SYSA'),
    ];
    expect(exportName(parts, null, null, 'America/New_York', 'csv')).toBe(
      '20310701_130000-20310701_143000_ScannerPulse_TG-7-SYSA,9-SYSB.csv',
    );
    expect(exportName(parts, new Date('2031-07-01T16:00:00Z'), null, 'UTC', 'pdf').startsWith('20310701_160000-')).toBe(true);
    expect(exportName([], null, null, 'UTC', 'csv')).toBe('ScannerPulse.csv');
  });

  it('shows five talkgroups and counts the rest, and stays at most 200 bytes', () => {
    const many = Array.from({ length: 12 }, (_, n) => part('2031-07-01T17:00:00Z', n + 1, 'SYSA'));
    expect(exportName(many, null, null, 'UTC', 'csv')).toBe(
      '20310701_170000-20310701_170000_ScannerPulse_TG-1-SYSA,2-SYSA,3-SYSA,4-SYSA,5-SYSA,+7.csv',
    );
    const name = exportName([part('2031-07-01T17:00:00Z', 1, 'É'.repeat(300))], null, null, 'UTC', 'csv');
    expect(new TextEncoder().encode(name).length).toBeLessThanOrEqual(200);
    expect(name.endsWith('.csv')).toBe(true);
  });

  it('uses UTC for a zone the browser does not know', () => {
    expect(exportName([part('2031-07-01T17:00:00Z', 1, 'S')], null, null, 'Nowhere/Else', 'csv')).toBe(
      '20310701_170000-20310701_170000_ScannerPulse_TG-1-S.csv',
    );
  });
});

describe('a CSV of the page', () => {
  it('quotes what needs quoting, ends lines with CRLF and starts with a byte-order mark', () => {
    const text = csvText([['Talkgroup', 'Text'], ['Fire, North', 'He said "go"\nnow']]);
    expect(text).toBe('﻿Talkgroup,Text\r\n"Fire, North","He said ""go""\nnow"\r\n');
  });

  it('never lets a cell start a formula in a spreadsheet', () => {
    for (const bad of ['=1+1', '+1', '-1', '@SUM(A1)', '\t=1']) {
      expect(csvText([[bad]]).slice(1)).toBe(`'${bad}\r\n`);
    }
  });

  const call = (more: Partial<Call>): Call => ({
    id: 7, started_at: '2031-07-01T17:00:02Z', duration_ms: 2450, talkgroup_id: 1, talkgroup: 101,
    talkgroup_name: 'Fire', system: 'SYSA', site: 'North', radio_id: 42, radio_alias: null,
    transcript: 'Engine 7', origin: 'machine', ...more,
  });

  it('has the columns on screen, the time split into date and time in the site\'s zone', () => {
    const rows = pageTable([call({})], ['time', 'talkgroup', 'id', 'length', 'text'], 'America/Chicago', 'https://radio.example');
    expect(rows).toEqual([
      ['Date', 'Time', 'Talkgroup', 'Call ID', 'Length (s)', 'Text', 'Audio'],
      ['2031-07-01', '12:00:02', 'Fire', '7', '2.5', 'Engine 7', 'https://radio.example/audio/7'],
    ]);
  });
});

describe('the export requests', () => {
  const list = new URLSearchParams('talkgroups=1,2&system=3&from=2031-07-01T00:00:00.000Z&smart=true&size=100&after=abc&count=false');

  it('ask the server for every match of the list, without its page', () => {
    expect(exportQuery(list).toString()).toBe('talkgroups=1%2C2&system=3&from=2031-07-01T00%3A00%3A00.000Z');
  });

  it('send the list\'s filter as JSON, the system as a number, and the chosen ids when there are some', () => {
    expect(audioBody(list)).toEqual({ talkgroups: '1,2', system: 3, from: '2031-07-01T00:00:00.000Z' });
    expect(audioBody(list, [5, 9])).toEqual({ talkgroups: '1,2', system: 3, from: '2031-07-01T00:00:00.000Z', ids: [5, 9] });
  });

  it('read the file\'s name from the answer, the UTF-8 name first', () => {
    expect(nameFromHeader('attachment; filename="caf_.mp3"; filename*=UTF-8\'\'caf%C3%A9.mp3', 'x')).toBe('café.mp3');
    expect(nameFromHeader('attachment; filename="a.xlsx"', 'x')).toBe('a.xlsx');
    expect(nameFromHeader(null, 'fallback.xlsx')).toBe('fallback.xlsx');
  });
});
