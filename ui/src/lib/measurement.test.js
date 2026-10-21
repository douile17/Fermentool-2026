import { describe, expect, it } from 'vitest';
import { defaultMeasurement, fitLine, measurementProfile, pushStable, redoAllSummary, stableMean, withoutMeasurements } from './measurement.js';

// A 240 s measurement drawing `gPerS` from a bottle of 5029.3 g, read in
// 0.1 g steps, like run 178 of calibration #14.
const ticks = (gPerS, end = 240, w0 = 5029.3) =>
  Array.from({ length: end }, (_, i) => ({ elapsed_s: i + 1, weight_g: Math.round((w0 - gPerS * (i + 1)) * 10) / 10 }));

describe('fitLine', () => {
  it('finds the line through points', () => {
    const f = fitLine([[0, 1], [1, 3], [2, 5], [3, 7], [4, 9]]);
    expect(f.k).toBeCloseTo(2, 9);
    expect(f.b0).toBeCloseTo(1, 9);
  });
  it('needs 5 points', () => {
    expect(fitLine([[0, 1], [1, 2]])).toBeNull();
  });
});

describe('measurementProfile', () => {
  it('reads the flow as the slope, in grams collected', () => {
    const p = measurementProfile(ticks(0.0979), 0.1);
    expect(p.gPerMin).toBeCloseTo(5.874, 2);
    expect(p.endS).toBe(240);
    expect(p.pts[0][1]).toBe(0);
    expect(p.pts.at(-1)[1]).toBeCloseTo(23.4, 1);
    // collected rises: the fit's slope is positive
    expect(p.fit.k).toBeGreaterThan(0);
    expect(Math.abs(p.driftPct)).toBeLessThan(1);
  });

  it('reads a measurement pumped into the bottle the same way', () => {
    const into = ticks(-0.0979);
    expect(measurementProfile(into, 0.1).gPerMin).toBeCloseTo(5.874, 2);
    expect(measurementProfile(into, 0.1).pts.at(-1)[1]).toBeGreaterThan(23);
  });

  it('sees a flow that falls in the second half', () => {
    const t = ticks(0.1).map((x) => ({ ...x }));
    for (const x of t) if (x.elapsed_s > 120) x.weight_g = Math.round((5029.3 - 12 - 0.08 * (x.elapsed_s - 120)) * 10) / 10;
    expect(measurementProfile(t, 0.1).driftPct).toBeLessThan(-15);
  });

  it('gives no drift for a dry measurement, never a huge number', () => {
    // a few balance steps over the whole measurement
    const t = ticks(0).map((x, i) => ({ ...x, weight_g: 5029.3 - (i > 200 ? 0.3 : i > 100 ? 0.2 : 0.1) }));
    const p = measurementProfile(t, 0.1);
    expect(p?.driftPct ?? null).toBeNull();
  });

  it('has nothing to show when the balance kept one stale weight', () => {
    expect(measurementProfile(ticks(0), 0.1)).toBeNull();
    expect(measurementProfile([], 0.1)).toBeNull();
  });
});

describe('defaultMeasurement', () => {
  it('opens on the one being dealt with', () => {
    expect(defaultMeasurement([{ weighed: true }, { weighed: false }, { weighed: false }])).toBe(1);
  });
  it('opens on the last weighed once all three are', () => {
    expect(defaultMeasurement([{ weighed: true }, { weighed: true }, { weighed: true }])).toBe(2);
  });
});

describe('the weighing: mean of the stable readings', () => {
  const feed = (readings, buf = []) => readings.reduce((b, r) => pushStable(b, r), buf);

  it('reads a balance flickering 27.4 / 27.5 / 27.6 g as 27.5', () => {
    const buf = feed([27.5, 27.4, 27.6, 27.5, 27.6, 27.4].map((w, i) => ({ t: 1000 * i, w, stable: true })));
    expect(stableMean(buf).g).toBeCloseTo(27.5, 9);
  });

  it('keeps the last 5 s only', () => {
    const buf = feed([10, 10, 20, 20, 20, 20, 20, 20].map((w, i) => ({ t: 1000 * i, w, stable: true })));
    expect(stableMean(buf)).toEqual({ g: 20, n: 5 });
  });

  it('starts over when the balance moves or stops answering', () => {
    let buf = feed([1, 2, 3].map((w, i) => ({ t: 1000 * i, w, stable: true })));
    buf = pushStable(buf, { t: 3000, w: 9, stable: false });
    expect(stableMean(buf)).toBeNull();
    buf = feed([{ t: 4000, w: 5, stable: true }], pushStable(buf, { t: 3500, w: null, stable: true }));
    expect(stableMean(buf)).toEqual({ g: 5, n: 1 });
  });
});

describe('redo all three', () => {
  const bursts = [
    { slot: 2, run_id: 201, weight_g: 24.6 },
    { slot: 0, run_id: 205, weight_g: null },
    { slot: 1, run_id: 204, weight_g: 24.5 },
  ];

  it('says what is dropped, in slot order, weights only where weighed', () => {
    expect(redoAllSummary(bursts)).toEqual({ n: 3, weights: [24.5, 24.6], runs: [205, 204, 201] });
    expect(redoAllSummary([])).toEqual({ n: 0, weights: [], runs: [] });
  });

  it('drops the measurements, keeps the tube and the pump setup', () => {
    const d = { tubing_lot_id: '17297216', setpoint: 150, direction: 'ccw', target_min: 4, auto: true, bursts };
    const after = withoutMeasurements(d);
    expect(after.bursts).toEqual([]);
    expect(after).toMatchObject({ tubing_lot_id: '17297216', setpoint: 150, direction: 'ccw', target_min: 4 });
    expect(d.bursts).toHaveLength(3); // the draft passed in is untouched
  });
});
