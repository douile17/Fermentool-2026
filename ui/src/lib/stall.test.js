// The balance checks of a calibration burst (stall.js), on simulated bursts.
// Every case that went wrong on the bench is here: a burst must raise its
// alarm, a working one never, down to very slow lines.
import { describe, expect, it } from 'vitest';
import { burstTrouble, expectedMlMin, movedG, stallCheckAtS } from './stall.js';

// A fixed noise, so a run is the same every time: +-0.04 g around the step.
const noise = (t) => ((t * 7919) % 9) / 100 - 0.04;

/** The first alarm a burst raises, and when; null if none. */
function firstAlarm({ flow, weightAt, durS, trusted, resG = 0.1, density = 1 }) {
  const mlMin = expectedMlMin(flow);
  const checkAtS = stallCheckAtS({ mlMin, densityGPerMl: density, resG, burstS: durS });
  const samples = [];
  for (let t = 0; t <= durS; t++) {
    const w = Math.round(weightAt(t) / resG) * resG + (t ? noise(t) : 0);
    samples.push({ t, w });
    const r = burstTrouble({ samples, checkAtS, resG, mlMin, densityGPerMl: density, trusted });
    if (r) return { kind: r, at: t };
  }
  return null;
}

// 150 rpm on a tube calibrated at 0.0413 ml/min per rpm: 6.2 ml/min.
const CAL = { controlVar: 'rpm', setpoint: 150, mlPerRpm: 0.0413 };
const draws = (mlMin, from = 700) => (t) => from - (mlMin * t) / 60;

describe('a working burst raises nothing', () => {
  it('at the expected flow, weight falling (bottle on the balance)', () => {
    expect(firstAlarm({ flow: CAL, weightAt: draws(6.2), durS: 300, trusted: true })).toBeNull();
  });
  it('weight rising (beaker on the balance)', () => {
    expect(firstAlarm({ flow: CAL, weightAt: (t) => 100 + (6.2 * t) / 60, durS: 300, trusted: true })).toBeNull();
  });
  it('a tube 30 % below its earlier calibration', () => {
    expect(firstAlarm({ flow: CAL, weightAt: draws(4.3), durS: 120, trusted: true })).toBeNull();
  });
  it('0.05 ml/min for an hour', () => {
    const flow = { controlVar: 'ml_min', setpoint: 0.05 };
    expect(firstAlarm({ flow, weightAt: draws(0.05), durS: 3600, trusted: true })).toBeNull();
  });
  it('0.05 ml/min at half its flow, on a 0.01 g balance', () => {
    const flow = { controlVar: 'ml_min', setpoint: 0.05 };
    expect(firstAlarm({ flow, weightAt: draws(0.025), durS: 3600, trusted: true, resG: 0.01 })).toBeNull();
  });
  it('no calibration yet: the bore figure only, a real flow at 40 % of it', () => {
    const flow = { controlVar: 'rpm', setpoint: 150, innerMm: 0.8 };
    expect(firstAlarm({ flow, weightAt: draws(0.4 * 9.2), durS: 60, trusted: false })).toBeNull();
  });
});

describe('a burst that goes wrong raises its alarm', () => {
  it('a dead line from the start: still, at the 30 s grace', () => {
    expect(firstAlarm({ flow: CAL, weightAt: () => 700, durS: 300, trusted: true })).toEqual({ kind: 'still', at: 30 });
  });
  // A line that stops can read 'slow' first while the window still holds
  // some flow, then 'stopped': either is the alarm, what matters is when.
  const STOP_KINDS = ['slow', 'stopped', 'reversed'];
  it('the line pulled off the head at 90 s (bench, 2026-10-12)', () => {
    const w = (t) => (t < 90 ? 700 + (6.2 * t) / 60 : 700 + (6.2 * 90) / 60 - 0.05 * (t - 90));
    const r = firstAlarm({ flow: CAL, weightAt: w, durS: 300, trusted: true });
    expect(STOP_KINDS).toContain(r?.kind);
    expect(r.at).toBeLessThan(130);
  });
  it('a line that stops dead at 120 s', () => {
    const stops = (t) => 700 - (6.2 * Math.min(t, 120)) / 60;
    const r = firstAlarm({ flow: CAL, weightAt: stops, durS: 300, trusted: true });
    expect(STOP_KINDS).toContain(r?.kind);
    expect(r.at).toBeGreaterThan(120);
    expect(r.at).toBeLessThan(160);
  });
  it('a line off the pump head that only siphons, 2 g/min (run 150, 2026-10-12)', () => {
    expect(firstAlarm({ flow: CAL, weightAt: draws(2.0), durS: 60, trusted: true })?.kind).toBe('slow');
  });
  it('a slow line that stops after 20 min at 0.05 ml/min', () => {
    const flow = { controlVar: 'ml_min', setpoint: 0.05 };
    const stops = (t) => 700 - (0.05 * Math.min(t, 1200)) / 60;
    const r = firstAlarm({ flow, weightAt: stops, durS: 3600, trusted: true });
    expect(STOP_KINDS).toContain(r?.kind);
    expect(r.at).toBeGreaterThan(1200);
  });
});

describe('when the checks run', () => {
  it('never before the grace, never too close to the end', () => {
    expect(stallCheckAtS({ mlMin: 200, densityGPerMl: 1, resG: 0.1, burstS: 60 })).toBe(30);
    // a check that would come after the burst ends is left to the end-of-burst one
    expect(stallCheckAtS({ mlMin: 0.016, densityGPerMl: 1, resG: 0.1, burstS: 1800 })).toBeNull();
  });
  it('a moved balance is 3 steps and at least 0.2 g', () => {
    expect(movedG(0.1)).toBeCloseTo(0.3);
    expect(movedG(0.01)).toBe(0.2);
  });
  it('the expected flow: ml/min as set, rpm through a calibration, else the bore', () => {
    expect(expectedMlMin({ controlVar: 'ml_min', setpoint: 2 })).toBe(2);
    expect(expectedMlMin({ controlVar: 'rpm', setpoint: 100, mlPerRpm: 0.04 })).toBeCloseTo(4);
    expect(expectedMlMin({ controlVar: 'rpm', setpoint: 100, innerMm: 0.8 })).toBeCloseTo(4.096);
    expect(expectedMlMin({ controlVar: 'rpm', setpoint: 0 })).toBeNull();
  });
});
