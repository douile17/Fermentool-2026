import { describe, expect, it } from 'vitest';
import { mmss, primeBand, primeElapsedS } from './prime.js';

describe('mmss', () => {
  it('counts minutes and two-digit seconds', () => {
    expect(mmss(0)).toBe('0:00');
    expect(mmss(64.7)).toBe('1:04');
    expect(mmss(300)).toBe('5:00');
    expect(mmss(null)).toBe('0:00');
  });
});

describe('primeElapsedS', () => {
  const since = '2026-10-28T09:00:00Z';
  it('counts from when the pump started this way', () => {
    expect(primeElapsedS({ since }, Date.parse('2026-10-28T09:00:42Z'))).toBe(42);
  });
  it('is null when not priming, never negative', () => {
    expect(primeElapsedS(null, Date.now())).toBeNull();
    expect(primeElapsedS({ since }, Date.parse('2026-10-28T08:59:59Z'))).toBe(0);
  });
});

describe('primeBand', () => {
  it('priming wins, whatever else', () => {
    expect(primeBand({ priming: { since: 'x' }, primedS: 30, lineEmptied: true })).toBe('run');
  });
  it('primed here, until the next measurement', () => {
    expect(primeBand({ priming: null, primedS: 64, lineEmptied: false })).toBe('done');
  });
  // 27 Oct 2026: a measurement pumped the wrong way emptied the line.
  it('asks for it after a measurement went the wrong way', () => {
    expect(primeBand({ priming: null, primedS: null, lineEmptied: true })).toBe('ask');
  });
  it('offers it quietly otherwise', () => {
    expect(primeBand({ priming: null, primedS: null, lineEmptied: false })).toBe('plain');
  });
});
