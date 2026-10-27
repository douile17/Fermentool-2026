import { describe, expect, it } from 'vitest';
import { boundsLine, canChangeRate, checkRate, curveFact, holdChip, rateDialogText, rateLabel, rateMarks, rateSince, rateSteps, runAgainPicks, runAgainSeed } from './ratechange.js';

// Local times, whatever the machine's zone: the UI shows the PC's clock.
const at = (d, h, m, s = 0) => new Date(2026, 9, d, h, m, s).toISOString();

// Run 214 of the mockup: 5.000 ml/min from Mon 14:10, 3.000 from Mon 20:30
// (6 h 20 in), 4.000 from Tue 08:15 (18 h 05 in).
const run214 = {
  id: 214,
  kind: 'dosing',
  status: 'completed',
  started_at: at(26, 14, 10),
  control_var: 'ml_min',
  duration_s: 86400,
  curve: { mode: 'endpoints', start: 5, end: 5, params: { kind: 'constant' } },
  setpoint_changes: [
    { elapsed_s: 22800, value: 3, wall_time: at(26, 20, 30) },
    { elapsed_s: 65100, value: 4, wall_time: at(27, 8, 15) },
  ],
};
const unchanged = { ...run214, setpoint_changes: [] };

// A linear run, 1 → 5 ml/min over 25 h, held at 5 from 25 h, then 2.5 from 26 h 10.
const linear = {
  ...run214,
  curve: { mode: 'endpoints', start: 1, end: 5, params: { kind: 'linear' } },
  duration_s: 90000,
  setpoint_changes: [{ elapsed_s: 94200, value: 2.5, wall_time: at(27, 16, 20) }],
};

describe('rateSteps of a non-constant curve', () => {
  it('starts at the curve end, once the curve is done, then each change', () => {
    const s = rateSteps(linear);
    expect(s.map((x) => x.value)).toEqual([5, 2.5]);
    expect(s.map((x) => x.elapsedS)).toEqual([90000, 94200]);
    expect(s.map((x) => x.atStart)).toEqual([false, false]);
    expect(s[0].wallMs).toBe(Date.parse(run214.started_at) + 90000 * 1000);
    expect(s.map((x) => x.last)).toEqual([false, true]);
  });
  it('is the end value alone without a change', () => {
    expect(rateSteps({ ...linear, setpoint_changes: [] })).toHaveLength(1);
    expect(rateSteps({ ...linear, setpoint_changes: [] })[0]).toMatchObject({ value: 5, elapsedS: 90000, atStart: false, last: true });
  });
});

describe('rateSteps', () => {
  it('gives the start rate, then each change, the last one marked', () => {
    const s = rateSteps(run214);
    expect(s.map((x) => x.value)).toEqual([5, 3, 4]);
    expect(s.map((x) => x.atStart)).toEqual([true, false, false]);
    expect(s.map((x) => x.last)).toEqual([false, false, true]);
    expect(s[0].wallMs).toBe(Date.parse(run214.started_at));
    expect(s[1].wallMs).toBe(Date.parse(at(26, 20, 30)));
    expect(s.map((x) => x.elapsedS)).toEqual([0, 22800, 65100]);
  });
  it('is one step, at the start and the last, without a change', () => {
    expect(rateSteps(unchanged)).toEqual([{ value: 5, wallMs: Date.parse(run214.started_at), elapsedS: 0, atStart: true, last: true }]);
    // a daemon before the rate change sends no list at all
    const old = { ...run214 };
    delete old.setpoint_changes;
    expect(rateSteps(old)).toHaveLength(1);
  });
});

describe('rateLabel', () => {
  it('says the day and the time of a step', () => {
    const s = rateSteps(run214);
    expect(s.map((x) => rateLabel(x))).toEqual(['Mon 14:10', 'Mon 20:30', 'Tue 08:15']);
  });
});

describe('rateSince', () => {
  it('says since when the rate holds', () => {
    expect(rateSince(unchanged)).toBe('constant since the start');
    expect(rateSince(run214)).toBe('since Tue 08:15');
  });
  it('says a hold in run time, "holding since 26 h 10"', () => {
    expect(rateSince(linear, true)).toBe('holding since 26 h 10');
  });
});

describe('rateDialogText', () => {
  it('words a Constant run and a hold', () => {
    expect(rateDialogText(run214, false)).toEqual({ title: 'New constant rate', scope: 'From now until the end of the run, or the next change.' });
    expect(rateDialogText(linear, true)).toEqual({ title: 'New hold rate', scope: 'From now until you stop the run, or the next change.' });
  });
});

describe('holdChip', () => {
  it('is the last held rate of a non-constant run with changes', () => {
    expect(holdChip(linear)).toEqual({ value: '2.500', unit: 'ml/min' });
  });
  it('is nothing for a Constant run or a run with no change', () => {
    expect(holdChip(run214)).toBeNull();
    expect(holdChip({ ...linear, setpoint_changes: [] })).toBeNull();
    expect(holdChip({ ...linear, setpoint_changes: undefined })).toBeNull();
  });
});

describe('runAgainPicks', () => {
  it('offers the rates only for a Constant run with changes', () => {
    expect(runAgainPicks(run214)).toBe(true);
    expect(runAgainPicks(unchanged)).toBe(false);
    expect(runAgainPicks(linear)).toBe(false);
    const exp = { ...linear, curve: { start: 1, end: 5, params: { kind: 'exponential' } } };
    expect(runAgainPicks(exp)).toBe(false);
  });
});

describe('curveFact', () => {
  it('names a Constant run by its length, not its first rate', () => {
    const r = { ...run214, duration_s: 86400 };
    expect(curveFact(r)).toBe('Constant, 24 h');
    expect(curveFact({ ...r, duration_s: 5400 })).toBe('Constant, 1 h 30 m');
    expect(curveFact({ ...r, duration_s: 600 })).toBe('Constant, 10 m');
  });
  it('keeps start → end for any other curve', () => {
    const lin = { control_var: 'ml_min', duration_s: 3600, curve: { start: 5, end: 50, params: { kind: 'linear' } } };
    expect(curveFact(lin)).toBe('5.000 → 50.000');
    expect(curveFact({ ...lin, control_var: 'rpm' })).toBe('5.0 → 50.0');
  });
});

describe('canChangeRate', () => {
  const status = (over = {}) => ({ active: { run_id: 214, kind: 'dosing', rate_bounds: [0, 99999], ...over } });
  it('is a Constant feed run, the active one', () => {
    expect(canChangeRate(status(), run214)).toBe(true);
  });
  it('is a linear run in its hold, where the daemon gives bounds', () => {
    expect(canChangeRate(status(), linear)).toBe(true);
    // still inside its curve: the daemon gives no bounds
    expect(canChangeRate(status({ rate_bounds: null }), linear)).toBe(false);
  });
  it('is not a calibration, another run or no run', () => {
    expect(canChangeRate(status({ kind: 'calibration' }), run214)).toBe(false);
    expect(canChangeRate(status({ run_id: 213 }), run214)).toBe(false);
    expect(canChangeRate({ active: null }, run214)).toBe(false);
    expect(canChangeRate(null, run214)).toBe(false);
    expect(canChangeRate(status(), null)).toBe(false);
  });
  it('is not a run the daemon gives no bounds for', () => {
    expect(canChangeRate(status({ rate_bounds: null }), run214)).toBe(false);
  });
});

describe('checkRate', () => {
  const b = [0, 14.27];
  it('takes a rate inside the bounds, other than the current one', () => {
    expect(checkRate(3, b, 5)).toBeNull();
    expect(checkRate(14.27, b, 5)).toBeNull();
  });
  it('refuses no value, zero or less, and out of bounds', () => {
    expect(checkRate(null, b, 5)).toBe('Enter the new rate.');
    expect(checkRate(Number.NaN, b, 5)).toBe('Enter the new rate.');
    expect(checkRate(0, b, 5)).toBe('The rate must be more than 0.');
    expect(checkRate(-1, b, 5)).toBe('The rate must be more than 0.');
    expect(checkRate(14.3, b, 5)).toBe('At most 14.270 ml/min on this run.');
    expect(checkRate(0.05, [0.1, 350], 5, 'rpm')).toBe('At least 0.1 rpm on this run.');
  });
  it('refuses the rate it already runs at, as shown', () => {
    expect(checkRate(5, b, 5)).toBe('The rate is already 5.000 ml/min.');
    expect(checkRate(5.0004, b, 5)).toBe('The rate is already 5.000 ml/min.');
    expect(checkRate(5.001, b, 5)).toBeNull();
  });
  it('without bounds still refuses zero', () => {
    expect(checkRate(0, null, 5)).toBe('The rate must be more than 0.');
    expect(checkRate(200, null, 5)).toBeNull();
  });
});

describe('boundsLine', () => {
  it('says what the rate may be set to', () => {
    expect(boundsLine([0, 14.27], 'ml_min')).toBe('More than 0, at most 14.270 ml/min on this run');
    expect(boundsLine([0.1, 350], 'rpm')).toBe('0.1 to 350.0 rpm on this run');
    expect(boundsLine(null, 'rpm')).toBe('');
  });
});

describe('rateMarks', () => {
  it('numbers each change, at its run time, under the chart', () => {
    expect(rateMarks(run214)).toEqual([
      { t: 22800, text: 'at 6 h 20, rate changed to 3.000 ml/min' },
      { t: 65100, text: 'at 18 h 05, to 4.000 ml/min' },
    ]);
    expect(rateMarks(unchanged)).toEqual([]);
  });
});

describe('runAgainSeed', () => {
  it('puts the chosen rate in a constant curve', () => {
    const seed = { name: 'x', kind: 'constant', start: 5, end: 5, value: 5, duration_h: 24 };
    expect(runAgainSeed(seed, 3)).toEqual({ name: 'x', kind: 'constant', start: 3, end: 3, value: 3, duration_h: 24 });
  });
});
