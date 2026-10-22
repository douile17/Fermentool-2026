import { describe, expect, it } from 'vitest';
import { bottleWeights, weighedOutShown, balanceLine } from './balance.js';

describe('bottleWeights', () => {
  // Run 182: 4394.7 g, refilled at 4094.7 g up to 4388.0 g while the feed
  // drew ~24 g, 4374.3 g now. The panel read 313.7 g out against 337.6 mL
  // delivered, and +293.3 g refilled for ~317 g poured.
  const report = {
    weight_start_g: 4394.7,
    weight_end_g: 4374.3,
    refills: [{ t_s: 1575.2, before_g: 4094.7, after_g: 4388.0, drawn_g: 23.9 }],
  };

  it('counts what the feed drew during a refill as weighed out', () => {
    const b = bottleWeights(report);
    expect(b.weighedOut).toBeCloseTo(300 + 23.9 + 13.7, 6);
    expect(b.refills[0].added_g).toBeCloseTo(293.3 + 23.9, 6);
  });

  it('reads a report from before drawn_g as the balance alone', () => {
    const old = { ...report, refills: [{ t_s: 1575.2, before_g: 4094.7, after_g: 4388.0 }] };
    const b = bottleWeights(old);
    expect(b.weighedOut).toBeCloseTo(313.7, 6);
    expect(b.refills[0].added_g).toBeCloseTo(293.3, 6);
  });
});

describe('weighedOutShown', () => {
  // Run 194, 08:33, mid third refill: two refills journalled, the bottle at
  // 3978.1 g and rising. The panel's own count read ~2870 g; the carboy
  // weighed 3219 g, the daemon 3221.9 g.
  const report = {
    weight_start_g: 4656.8,
    weight_end_g: 3983.0,
    refills: [
      { t_s: 18819, before_g: 3656.8, after_g: 4655.1, drawn_g: 98.0 },
      { t_s: 37578, before_g: 3656.7, after_g: 4655.3, drawn_g: 95.9 },
    ],
  };
  const live = 3978.1;
  const bottleOut = bottleWeights(report).weighedOut + (report.weight_end_g - live);

  it('reproduces the wrong count mid-refill', () => {
    expect(bottleOut).toBeCloseTo(2869.6, 0);
  });
  it("shows the daemon's figure while a refill runs, said estimated", () => {
    expect(weighedOutShown({ bottleOut, deliveredG: 3221.9, refilling: true })).toEqual({ g: 3221.9, estimated: true });
  });
  it("keeps the panel's own count outside a refill", () => {
    expect(weighedOutShown({ bottleOut: 3305.4, deliveredG: 3305.6, refilling: false })).toEqual({ g: 3305.4, estimated: false });
  });
  it('falls back to its own count when the daemon has no figure', () => {
    expect(weighedOutShown({ bottleOut, deliveredG: null, refilling: true }).estimated).toBe(false);
  });
});

describe('balanceLine', () => {
  it('says the weight when the balance answers, quiet', () => {
    expect(balanceLine({ scale_connected: true, scale_weight_g: 4345.24, scale_stable: true }, 'COM7')).toEqual({ tone: '', word: 'Connected', text: 'COM7, 4345.2 g, stable' });
  });
  it('says what to check when it does not', () => {
    const l = balanceLine({ scale_connected: false }, 'COM7');
    expect(l.tone).toBe('bad');
    expect(l.word).toBe('Not answering');
    expect(l.text).toMatch(/^COM7: is the balance on/);
  });
  it('a port saved but closed by Disconnect says so, not an error', () => {
    expect(balanceLine({ scale_connected: null }, 'COM7')).toEqual({ tone: 'idle', word: 'Disconnected', text: 'COM7, not weighing' });
  });
  it('no port, no balance', () => {
    expect(balanceLine({}, '')).toMatchObject({ tone: 'idle', word: 'No balance' });
  });
});
