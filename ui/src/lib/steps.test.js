import { describe, expect, it } from 'vitest';
import {
  MAX_STEPS, parseStepRows, readCsv, runDurationS, spanLabel, splitAt, stepAt, stepLasts, stepsCheck, stepsCurve, stepsFromSegments, stepsProblem, templateSheets,
} from './steps.js';
import { readXlsx, writeXlsx } from './xlsx.js';

const S = (h, m, value) => ({ h: String(h), m: String(m), value });
const HEAD = ['From (h)', 'Rate (ml/min)'];
// Excel's "CSV UTF-8" starts with a byte order mark
const BOM = String.fromCharCode(0xfeff);

describe('stepAt / splitAt', () => {
  it('turns hours and minutes into seconds and back', () => {
    expect(stepAt(S(12, 30, 1))).toBe(45000);
    expect(stepAt({ h: '', m: '', value: 1 })).toBe(0);
    expect(stepAt(S('x', 0, 1))).toBeNaN();
    expect(splitAt(45000)).toEqual({ h: '12', m: '30' });
    expect(splitAt(36)).toEqual({ h: '0', m: '0.6' });
  });
});

describe('stepsProblem', () => {
  it('accepts steps in order within the run', () => {
    expect(stepsProblem([S(0, 0, 0.8), S(6, 0, 1.5), S(12, 30, 0)], 48 * 3600)).toBeNull();
  });
  it('says which step is out of order, missing or past the end', () => {
    expect(stepsProblem([], 3600)).toBe('Add at least one step.');
    expect(stepsProblem([S(0, 0, 1), S(6, 0, 2), S(6, 0, 3)], 86400)).toBe(
      'Step 3 starts at or before step 2: each step starts later than the one before.',
    );
    expect(stepsProblem([S(0, 0, 1), S(6, 0, -1)], 86400)).toBe('Step 2: give a rate of 0 or more.');
    expect(stepsProblem([S(0, 0, 1), S(6, 0, '')], 86400)).toBe('Step 2: give a rate of 0 or more.');
    expect(stepsProblem([S(0, 0, 1), S(30, 0, 2)], 86400)).toBe('Step 2 starts after the run ends: lengthen the duration.');
  });
  it('points at the step typed back in time, 4th row before the 3rd', () => {
    const steps = [S(0, 0, 1), S(6, 0, 2), S(12, 0, 3), S(2, 0, 4), S(30, 0, 5)];
    expect(stepsCheck(steps, 48 * 3600)).toEqual({
      i: 3, field: 'time', text: 'Step 4 starts at or before step 3: each step starts later than the one before.',
    });
  });
  it('leaves the end to the duration check while there is none', () => {
    expect(stepsProblem([S(0, 0, 1), S(30, 0, 2)], 0)).toBeNull();
  });
});

describe('stepsCurve / stepsFromSegments', () => {
  it('makes the daemon segments, the first at 0 whatever was typed', () => {
    expect(stepsCurve([S(3, 0, 0.8), S(6, 0, 1.5)])).toEqual({
      start: 0.8, end: 1.5, segments: [{ at_seconds: 0, value: 0.8 }, { at_seconds: 21600, value: 1.5 }],
    });
  });
  it('brings a run\'s segments back as form steps, Run again', () => {
    const steps = [S(0, 0, 0.8), S(12, 30, 2.5)];
    expect(stepsFromSegments(stepsCurve(steps).segments)).toEqual(steps);
  });
  it('says a length in few words', () => {
    expect(spanLabel(21600)).toBe('6 h');
    expect(spanLabel(23400)).toBe('6 h 30');
    expect(spanLabel(21900)).toBe('6 h 05');
    expect(spanLabel(2700)).toBe('45 min');
    expect(spanLabel(0)).toBe('');
  });
  it('says how long each step lasts, the last to the end', () => {
    const steps = [S(0, 0, 1), S(6, 0, 2)];
    expect(stepLasts(steps, 0, 86400)).toBe(21600);
    expect(stepLasts(steps, 1, 86400)).toBe(64800);
  });
});

describe('parseStepRows', () => {
  const ml = { unit: 'ml/min' };
  it('takes the template\'s rows, empty rows at the end ignored', () => {
    expect(parseStepRows([HEAD, [0, 0.8], [12.5, 2.5], [], [null, null]], ml)).toEqual({ steps: [S(0, 0, 0.8), S(12, 30, 2.5)] });
  });
  it('refuses other headers, and a file in the other unit', () => {
    expect(parseStepRows([['Time', 'Rate (ml/min)'], [0, 1]], ml).errors[0]).toMatch(/column A must be headed "From \(h\)"/);
    expect(parseStepRows([['From (h)', 'Rate (rpm)'], [0, 1]], ml).errors).toEqual([
      'This file is in rpm, the run is in ml/min. Download the ml/min template.',
    ]);
  });
  it('lists every bad row with its Excel row number', () => {
    const { errors } = parseStepRows(
      [HEAD, [0, 1], [12, 2], [12, 3], [], [{ time: 0.52 }, 1], ['6h', 1], [20, 'fast'], [21, -1], [22, null]],
      ml,
    );
    expect(errors).toEqual([
      "Row 4: 12 h is not after row 3's 12 h.",
      'Row 5 is empty.',
      'Row 6: column A is a time of day; give hours as a number, 12.5 for 12 h 30 min.',
      'Row 7: "6h" in column A is not a number of hours.',
      'Row 8: "fast" in column B is not a rate.',
      'Row 9: -1 ml/min is negative.',
      'Row 10: no rate in column B.',
    ]);
  });
  it('wants the first step at 0 h', () => {
    expect(parseStepRows([HEAD, [1, 1]], ml).errors).toEqual(['Row 2: the first step must start at 0 h.']);
  });
  it('refuses a rate past what the pump and tube deliver', () => {
    const r = parseStepRows([HEAD, [0, 1], [6, 18.5]], { unit: 'ml/min', max: 14.2, maxWhy: 'this tube delivers (14.2 ml/min at 350 rpm)' });
    expect(r.errors).toEqual(['Row 3: 18.5 ml/min is more than this tube delivers (14.2 ml/min at 350 rpm).']);
  });
  it('refuses an empty file and one past the limit', () => {
    expect(parseStepRows([HEAD], ml).errors).toEqual(['The file has no steps under its headers.']);
    const many = [HEAD, ...Array.from({ length: MAX_STEPS + 1 }, (_, i) => [i, 1])];
    expect(parseStepRows(many, ml).errors).toEqual([`${MAX_STEPS + 1} steps: at most ${MAX_STEPS}.`]);
  });
});

describe('readCsv', () => {
  it('reads ";" with decimal commas, and "," with decimal points', () => {
    expect(readCsv(BOM + 'From (h);Rate (ml/min)\r\n0;0,8\r\n12,5;2,5\r\n')).toEqual([HEAD, [0, 0.8], [12.5, 2.5], [null]]);
    expect(readCsv('From (h),Rate (ml/min)\n0,0.8\n12.5,2.5')).toEqual([HEAD, [0, 0.8], [12.5, 2.5]]);
  });
});

describe('the template', () => {
  it('imports as it is downloaded, its examples as steps', async () => {
    const rows = await readXlsx(writeXlsx(templateSheets('ml/min')));
    expect(parseStepRows(rows, { unit: 'ml/min' })).toEqual({ steps: [S(0, 0, 0.8), S(6, 0, 1.5), S(12, 30, 2.5), S(24, 0, 4)] });
  });
  it('the rpm one is refused on an ml/min run', async () => {
    const rows = await readXlsx(writeXlsx(templateSheets('rpm')));
    expect(parseStepRows(rows, { unit: 'ml/min' }).errors[0]).toMatch(/This file is in rpm/);
  });
});

describe('runDurationS', () => {
  it('ignores seconds left from another shape in a Steps run', () => {
    expect(runDurationS({ kind: 'step', duration_h: '2', duration_m: '0', duration_s: '30' })).toBe(7200);
    expect(runDurationS({ kind: 'linear', duration_h: '2', duration_m: '0', duration_s: '30' })).toBe(7230);
  });
});
