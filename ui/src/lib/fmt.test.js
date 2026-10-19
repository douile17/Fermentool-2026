// Formatting (fmt.js): what History and the Overview print.
import { describe, expect, it } from 'vitest';
import { ranFor, stamp } from './fmt.js';

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
