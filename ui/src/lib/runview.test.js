import { describe, expect, it } from 'vitest';
import { deliveryFigures, flowWindowS, inAbout, nextRefill, refillDueInS, refillProgress, runState, stateTitle, weighedFlow } from './runview.js';

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

describe('runState', () => {
  const run = { active: { gravimetric_trim: true, auto_refill: true }, scale_state: 'normal', scale_ok: true };
  const cycle = (o) => ({ refill: { cycle: { phase: 'idle', halted: false, low_g: 3063, high_g: 5063, weight_g: 4000, flow_ml_min: 300, ...o } } });

  it('tells feeding from refilling at a glance: colour, word and picture differ', () => {
    const feeding = runState({ ...run, ...cycle() }, true, false);
    const filling = runState({ ...run, ...cycle({ phase: 'filling', weight_g: 3903 }) }, true, false);
    expect(feeding).toMatchObject({ tone: 'ok', word: 'Feeding', icon: 'pump' });
    expect(filling).toMatchObject({ tone: 'fill', word: 'Refilling', icon: 'fill' });
  });
  it('shows a full bottle settling with its own picture', () => {
    expect(runState({ ...run, ...cycle({ phase: 'settling' }) }, true, false)).toMatchObject({ tone: 'fill', word: 'Settling', icon: 'full' });
    expect(runState({ ...run, scale_state: 'refill_settling' }, true, false).icon).toBe('full');
  });
  it('a stopped automatic refill needs a look (run 186: the refill pump gained 2 g of 146)', () => {
    expect(runState({ ...run, ...cycle({ halted: true }) }, true, false)).toMatchObject({ tone: 'warn', word: 'Refill stopped', icon: 'bottle-warn' });
  });
  it('an alarm still wins over a refill', () => {
    const s = { ...run, ...cycle({ phase: 'filling' }), tracking: { alarm: 'feed_stopped' } };
    expect(runState(s, true, false)).toMatchObject({ tone: 'bad', word: 'Alarm', title: 'Feed stopped' });
  });
  it('a silent daemon reads as no news, whatever it said last', () => {
    expect(runState({ ...run, ...cycle({ phase: 'filling' }) }, false, false).tone).toBe('idle');
  });
});

describe('refillProgress', () => {
  it('goes from the refill level to the full bottle, with the time left at the refill flow', () => {
    const p = refillProgress({ phase: 'filling', low_g: 3063, high_g: 5063, weight_g: 3903, flow_ml_min: 290 });
    expect(p.frac).toBeCloseTo(0.42, 2);
    expect(p.leftMin).toBeCloseTo(4, 0);
    expect(p.toG).toBe(5063);
  });
  it('is nothing when no fill is under way, and stays in 0..1 for a Refill now above the level', () => {
    expect(refillProgress({ phase: 'idle', low_g: 0, high_g: 1, weight_g: 0.5 })).toBeNull();
    expect(refillProgress({ phase: 'filling', low_g: 3063, high_g: 5063, weight_g: 2580, flow_ml_min: 0 })).toMatchObject({ frac: 0, leftMin: null });
  });
});

describe('stateTitle', () => {
  it('puts the state first in the window title', () => {
    expect(stateTitle({ tone: 'ok', word: 'Feeding', title: 'Running normally' }, '8.33 ml/min')).toBe('Feeding 8.33 ml/min · Fermentool');
    expect(stateTitle({ tone: 'fill', word: 'Refilling', progress: { frac: 0.42 } })).toBe('Refilling 42 % · Fermentool');
    expect(stateTitle({ tone: 'bad', word: 'Alarm', title: 'Feed stopped' })).toBe('Feed stopped · Fermentool');
    expect(stateTitle(null)).toBe('Fermentool');
  });
});

describe('nextRefill', () => {
  const cycle = { phase: 'idle', halted: false, low_g: 2562.1, high_g: 4562.1, weight_g: 2840 };
  const nowMs = Date.parse('2026-10-19T12:31:00Z');
  it('says when and between which levels, not a task with nothing to do', () => {
    const n = nextRefill({ cycle, inS: 33 * 60, nowMs });
    expect(n.label).toBe('Next refill');
    expect(n.what).toMatch(/^around /);
    expect(n.sub).toBe('in about 33 min, at 2.56 kg, back to 4.56 kg');
  });
  it('gives the level when the balance cannot time it yet', () => {
    expect(nextRefill({ cycle, inS: null, nowMs }).what).toBe('under 2.56 kg');
  });
  it('is nothing while a fill runs or the cycle is stopped', () => {
    expect(nextRefill({ cycle: { ...cycle, phase: 'filling' }, inS: 10, nowMs })).toBeNull();
    expect(nextRefill({ cycle: { ...cycle, halted: true }, inS: 10, nowMs })).toBeNull();
  });
});

describe('inAbout', () => {
  it('reads in minutes under an hour, in hours past it', () => {
    expect(inAbout(35 * 60)).toBe('in about 35 min');
    expect(inAbout(20)).toBe('in about 1 min');
    expect(inAbout(2 * 3600 + 5 * 60)).toBe('in 2 h 05');
  });
});

describe('deliveryFigures', () => {
  it('reads as delivered, not as a bare 0 % (run 192: 1.722 L of 1.721 L)', () => {
    const d = deliveryFigures([12000, 1721, 1722]);
    expect(d.deliveredPct).toBeCloseTo(100.06, 2);
    expect(d.gapPct).toBeCloseTo(0.06, 2);
  });
  it('is nothing before anything is asked', () => {
    expect(deliveryFigures([0, 0, 0])).toBeNull();
    expect(deliveryFigures(null)).toBeNull();
  });
});
