import { describe, expect, it } from 'vitest';
import { bottleWeights } from './balance.js';

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
