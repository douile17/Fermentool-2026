// A calibrated tube's real flow range (tuberange.js): New run warns before
// the daemon refuses a curve the tube cannot deliver.
import { describe, expect, it } from 'vitest';
import { curveFit, mlPerRpmOf, tubeRange } from './tuberange.js';

// Calibration 14 of 2026-10-15: 0.8 mm tube, 5.83 ml/min at 150 rpm.
const CAL = { control_var: 'rpm', setpoint: 150, mean_measured_ml_min: 5.831 };

describe('mlPerRpmOf', () => {
  it('reads an rpm calibration', () => {
    expect(mlPerRpmOf(CAL)).toBeCloseTo(0.03887, 5);
  });
  it('gives nothing for an ml/min calibration or a broken one', () => {
    expect(mlPerRpmOf({ ...CAL, control_var: 'ml_min' })).toBeNull();
    expect(mlPerRpmOf({ ...CAL, setpoint: 0 })).toBeNull();
    expect(mlPerRpmOf(null)).toBeNull();
  });
});

describe('tubeRange', () => {
  it('is 0.1 to 350 rpm in real ml/min', () => {
    const r = tubeRange(mlPerRpmOf(CAL));
    expect(r.min).toBeCloseTo(0.00389, 4);
    expect(r.max).toBeCloseTo(13.6, 1);
  });
});

describe('curveFit', () => {
  const mlPerRpm = mlPerRpmOf(CAL);
  it('50 ml/min on a 0.8 mm tube is over (the 0x03 of 2026-10-15)', () => {
    const fit = curveFit({ peak: 50, mlPerRpm });
    expect(fit.over).toBe(true);
    expect(fit.rpm).toBeGreaterThan(1200);
  });
  it('10 ml/min fits, with or without the regulation', () => {
    expect(curveFit({ peak: 10, mlPerRpm }).over).toBe(false);
    expect(curveFit({ peak: 10, mlPerRpm, trimLimitPct: 25 }).tight).toBe(false);
  });
  it('13 ml/min fits but leaves the regulation no headroom', () => {
    const fit = curveFit({ peak: 13, mlPerRpm, trimLimitPct: 10 });
    expect(fit.over).toBe(false);
    expect(fit.tight).toBe(true);
    expect(fit.safeMax).toBeCloseTo(12.37, 1);
  });
  it('the exact maximum is not over', () => {
    expect(curveFit({ peak: 350 * mlPerRpm, mlPerRpm }).over).toBe(false);
  });
  it('no calibration, no verdict', () => {
    expect(curveFit({ peak: 50, mlPerRpm: null })).toBeNull();
  });
});

describe('maxRpm', () => {
  const mlPerRpm = mlPerRpmOf(CAL);
  it('a LabV capped at 150 rpm delivers less', () => {
    expect(tubeRange(mlPerRpm, 150).max).toBeCloseTo(5.83, 1);
    expect(tubeRange(mlPerRpm).max).toBeCloseTo(13.6, 1);
  });
  it('the fit uses it', () => {
    const fit = curveFit({ peak: 10, mlPerRpm, maxRpm: 150 });
    expect(fit.over).toBe(true);
    expect(fit.maxRpm).toBe(150);
    expect(curveFit({ peak: 10, mlPerRpm, maxRpm: 600 }).over).toBe(false);
  });
});
