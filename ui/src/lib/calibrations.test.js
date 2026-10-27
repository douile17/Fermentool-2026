import { describe, expect, it } from 'vitest';
import { forFeedPump } from './calibrations.js';

describe('forFeedPump', () => {
  it('keeps the calibrations of the feed pump model, a row without one counts as LabQ', () => {
    const cals = [{ id: 1 }, { id: 2, pump_model: 'labv' }, { id: 3, pump_model: 'labq' }];
    expect(forFeedPump(cals, 'labq').map((c) => c.id)).toEqual([1, 3]);
    expect(forFeedPump(cals, 'labv').map((c) => c.id)).toEqual([2]);
    expect(forFeedPump(null, 'labq')).toEqual([]);
    expect(forFeedPump(cals, undefined).map((c) => c.id)).toEqual([1, 3]);
  });
});
