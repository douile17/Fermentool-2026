// The chart's value axis (chartscale.js): from zero, with round graduations.
import { describe, expect, it } from 'vitest';
import { niceStep, valueScale, elapsedTicks, ticksFor } from './chartscale.js';

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

describe('elapsedTicks', () => {
  const labels = (span) => elapsedTicks(span).map((k) => k.label);
  it('in minutes for a short curve', () => {
    expect(labels(20 * 60)).toEqual(['0', '5 min', '10 min', '15 min', '20 min']);
    expect(labels(90 * 60)).toEqual(['0', '15 min', '30 min', '45 min', '60 min', '75 min', '90 min']);
  });
  it('in hours past 2 h, the step growing with the run', () => {
    expect(labels(6 * 3600)).toEqual(['0', '1 h', '2 h', '3 h', '4 h', '5 h', '6 h']);
    expect(labels(24 * 3600)).toEqual(['0', '4 h', '8 h', '12 h', '16 h', '20 h', '24 h']);
    expect(labels(100 * 3600)).toEqual(['0', '24 h', '48 h', '72 h', '96 h']);
  });
  it('never more than six intervals, nothing for no span', () => {
    for (const span of [61, 3000, 7200, 7201, 50 * 3600, 300 * 3600]) expect(elapsedTicks(span).length).toBeLessThanOrEqual(7);
    expect(elapsedTicks(0)).toEqual([]);
  });
});

describe('ticksFor', () => {
  it('fewer labels on a narrow chart, so they never overlap', () => {
    // New run's preview, ~360 px: a 2 h curve read 100 min and 120 min on top of each other
    expect(elapsedTicks(7200, ticksFor(360)).map((k) => k.label)).toEqual(['0', '30 min', '60 min', '90 min', '120 min']);
    expect(ticksFor(1000)).toBe(6);
    expect(ticksFor(100)).toBe(2);
  });
});
