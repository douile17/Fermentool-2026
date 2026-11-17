import { describe, expect, it } from 'vitest';
import { newestOnly, previewWanted, profileReady } from './preview.js';

describe('newestOnly', () => {
  it('keeps only the answer to the latest request', () => {
    const req = newestOnly();
    const a = req.begin();
    const b = req.begin();
    expect(req.current(a)).toBe(false);
    expect(req.current(b)).toBe(true);
  });
});

describe('profileReady', () => {
  const drawn = { preview: [[0, 1], [60, 1]], previewErr: null, durationS: 60 };
  it('is ready with a curve drawn and nothing wrong', () => {
    expect(profileReady(drawn)).toBe(true);
  });
  it('is not ready while a step is out of order, even with a curve drawn', () => {
    expect(profileReady({ ...drawn, stepsErr: 'Step 4 starts at or before step 3' })).toBe(false);
  });
  it('is not ready without a curve, a duration, or with an error', () => {
    expect(profileReady({ ...drawn, preview: [] })).toBe(false);
    expect(profileReady({ ...drawn, durationS: 0 })).toBe(false);
    expect(profileReady({ ...drawn, previewErr: 'bad' })).toBe(false);
  });
});

describe('previewWanted', () => {
  it('asks only with a duration and steps that hold', () => {
    expect(previewWanted({ durationS: 0 })).toBe(false);
    expect(previewWanted({ durationS: 3600, stepsErr: 'Step 2 starts before step 1' })).toBe(false);
    expect(previewWanted({ durationS: 3600 })).toBe(true);
  });
});
