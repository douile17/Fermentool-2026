// The chart's value axis (chartscale.js): from zero, with round graduations.
import { describe, expect, it } from 'vitest';
import { niceStep, valueScale } from './chartscale.js';

describe('niceStep', () => {
  it('rounds to 1, 2, 2.5 or 5 times a power of ten', () => {
    expect(niceStep(0.7)).toBe(1);
    expect(niceStep(1.6)).toBe(2);
    expect(niceStep(2.2)).toBe(2.5);
    expect(niceStep(3)).toBe(5);
    expect(niceStep(7)).toBe(10);
    expect(niceStep(180)).toBe(200);
  });
});

describe('valueScale', () => {
  it('does not zoom onto a flat run weighed within 1 % (run 186)', () => {
    const s = valueScale([8.33, 8.33, 8.237, 8.431, 8.365]);
    expect(s.lo).toBe(0);
    expect(s.hi).toBe(10);
    expect(s.ticks).toEqual([0, 2.5, 5, 7.5, 10]);
    // the 1 % swing fills a few percent of the height, not all of it
    expect((8.431 - 8.237) / (s.hi - s.lo)).toBeLessThan(0.03);
  });
  it('tops a little above the highest value', () => {
    const s = valueScale([5, 100]);
    expect(s.lo).toBe(0);
    expect(s.hi).toBeGreaterThanOrEqual(105);
    expect(s.ticks.at(-1)).toBe(s.hi);
  });
  it('goes under zero only for a negative value', () => {
    const s = valueScale([-3, 10]);
    expect(s.lo).toBeLessThan(-3);
    expect(s.ticks).toContain(0);
  });
  it('still has an axis with nothing, or only zeros, to draw', () => {
    expect(valueScale([])).toEqual({ lo: 0, hi: 1.5, ticks: [0, 0.5, 1, 1.5] });
    expect(valueScale([0, 0]).hi).toBeGreaterThan(0);
  });
});
