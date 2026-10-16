import { describe, expect, it } from 'vitest';
import { flowWindowS, refillDueInS, weighedFlow } from './runview.js';

describe('weighedFlow', () => {
  // Run 184's curve: 13 -> 10 mL/min over 1 h, a point every 2 s.
  const planned = [[0, 13], [3600, 10]];
  const rate = (t) => 13 - (3 * t) / 3600;
  const points = (share) => {
    const out = [];
    let r = 0;
    for (let t = 0; t <= 3600; t += 2) {
      out.push([t, r, r * share]);
      r += (rate(t + 1) * 2) / 60;
    }
    return out;
  };

  it('follows the curve when the pump delivers what was asked', () => {
    const f = weighedFlow(points(1), planned);
    expect(f.length).toBeGreaterThan(100);
    expect(f.length).toBeLessThanOrEqual(151);
    for (const [t, v] of f) expect(v).toBeCloseTo(rate(t), 2);
  });

  it('reads under the curve when the pump delivers short', () => {
    const f = weighedFlow(points(0.98), planned);
    for (const [t, v] of f) expect(v / rate(t)).toBeCloseTo(0.98, 3);
  });

  it('starts once a whole window is weighed', () => {
    const f = weighedFlow(points(1), planned);
    expect(f[0][0]).toBeGreaterThanOrEqual(300);
    expect(weighedFlow([], planned)).toEqual([]);
  });

  it('averages over 10 min, or a 30th of a long run', () => {
    expect(flowWindowS(3600)).toBe(600);
    expect(flowWindowS(100 * 3600)).toBe(12000);
  });
});

describe('refillDueInS', () => {
  // 10 g/min flat: 1 ml/min setpoint, 10 g per minute per unit.
  const flow = { gPerUnit: 10, planned: [[0, 1], [36000, 1]], elapsedS: 0, durationS: 36000 };
  const cycle = { phase: 'idle', halted: false, low_g: 4000, high_g: 5000 };

  it('counts down to the refill level, not to an empty bottle', () => {
    // 600 g above the level at 10 g/min: one hour.
    expect(refillDueInS({ cycle, weightG: 4600, ...flow })).toBeCloseTo(3600, 0);
  });

  it('follows a rising curve', () => {
    const rising = { ...flow, planned: [[0, 1], [3600, 1], [3601, 2], [36000, 2]] };
    // 600 g used in the first hour, the next 600 g at 20 g/min (within the
    // one-minute integration step).
    const s = refillDueInS({ cycle, weightG: 5200, ...rising });
    expect(Math.abs(s - (3600 + 1800))).toBeLessThanOrEqual(60);
  });

  it('has nothing to show while no refill waits', () => {
    expect(refillDueInS({ cycle: null, weightG: 4600, ...flow })).toBeNull();
    expect(refillDueInS({ cycle: { ...cycle, halted: true }, weightG: 4600, ...flow })).toBeNull();
    expect(refillDueInS({ cycle: { ...cycle, phase: 'filling' }, weightG: 4600, ...flow })).toBeNull();
    expect(refillDueInS({ cycle, weightG: 3990, ...flow })).toBeNull();
    expect(refillDueInS({ cycle, weightG: null, ...flow })).toBeNull();
  });
});
