// Formatting (fmt.js): what History and the Overview print.
import { describe, expect, it } from 'vitest';
import { ranFor, stamp, insideFolder } from './fmt.js';

describe('ranFor', () => {
  const planned = { duration_s: 28800, started_at: '2026-10-19T07:53:25Z' };
  it('gives what a stopped run really ran, not its planned 8 h (run 188: 2 minutes)', () => {
    const r = ranFor({ ...planned, status: 'stopped', ended_at: '2026-10-19T07:55:25Z' });
    expect(r).toEqual({ ran: '2 m 0 s', of: 'of 8 h 0 m' });
  });
  it('counts a running run so far', () => {
    const r = ranFor({ ...planned, status: 'running', ended_at: null }, Date.parse('2026-10-19T11:20:25Z'));
    expect(r).toEqual({ ran: '3 h 27 m', of: 'of 8 h 0 m, so far' });
  });
  it('leaves out the plan for a completed run: it ran its whole curve', () => {
    const r = ranFor({ duration_s: 3600, started_at: '2026-10-15T14:19:45Z', status: 'completed', ended_at: '2026-10-15T15:19:52Z' });
    expect(r).toEqual({ ran: '1 h 0 m', of: null });
  });
});

describe('stamp', () => {
  it('is in English whatever the PC language', () => {
    expect(stamp('2026-10-19T09:04:00Z')).toMatch(/Oct/);
  });
});

describe('insideFolder', () => {
  // Windows paths, built from their parts: C:\Users\Drew\...
  const win = (...parts) => parts.join(String.fromCharCode(92));
  const data = win('C:', 'Users', 'Drew', 'AppData', 'Roaming', 'Fermentool');

  it('writes a file or a folder of the data folder by its own name', () => {
    expect(insideFolder(win(data, 'fermentool.sqlite'), data)).toBe('fermentool.sqlite');
    expect(insideFolder(win(data, 'logs'), data, true)).toBe(win('logs', ''));
  });

  it('is null outside it, or for a mere name prefix', () => {
    expect(insideFolder(win('D:', 'fermentool', 'config.toml'), data)).toBeNull();
    expect(insideFolder(win(data + '2', 'config.toml'), data)).toBeNull();
    expect(insideFolder(data, data)).toBeNull();
  });

  it('ignores case and a trailing separator', () => {
    expect(insideFolder(win(data.toLowerCase(), 'config.toml'), win(data, ''))).toBe('config.toml');
  });
});
