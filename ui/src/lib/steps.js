// A Steps feed profile: rates held from given times, typed in New run or
// read from a file made from the template. The daemon's Step curve holds
// each segment's value from its `at_seconds` to the next one, then the
// last one until Stop.
//
// A step in the form is `{ h, m, value }`: hours and minutes as typed
// (strings, like the duration fields), the rate a number.
//
// The file is checked whole before anything is taken: one bad row and
// nothing is imported, each problem said with its row number.

import { num } from './fmt.js';

// Past this the file is refused: a column dragged down to row 50 000
// would freeze the page for nothing.
export const MAX_STEPS = 1000;

export const HEAD_TIME = 'From (h)';
export const headRate = (unit) => `Rate (${unit})`;

/** A run's length, s, from New run's fields. A Steps run's End row has
 *  hours and minutes only: seconds typed under another shape, or brought by
 *  Run again, would lengthen it unseen. */
export function runDurationS({ kind, duration_h, duration_m, duration_s }) {
  const sec = kind === 'step' ? 0 : +duration_s || 0;
  return Math.max(0, Math.round((+duration_h || 0) * 3600 + (+duration_m || 0) * 60 + sec));
}

/** A step's start, in whole seconds, or NaN when not typed right. */
export function stepAt(step) {
  const h = String(step.h ?? '').trim();
  const m = String(step.m ?? '').trim();
  const hn = h === '' ? 0 : Number(h);
  const mn = m === '' ? 0 : Number(m);
  if (!Number.isFinite(hn) || !Number.isFinite(mn) || hn < 0 || mn < 0) return NaN;
  return Math.round(hn * 3600 + mn * 60);
}

/** Seconds -> `{ h, m }` strings: 45000 -> 12 h, 30 min; a fraction of a
 *  minute stays (0.01 h is 0.6 min). */
export function splitAt(s) {
  const h = Math.floor(s / 3600);
  const m = Math.round(((s - h * 3600) / 60) * 100) / 100;
  return { h: String(h), m: String(m) };
}

/** The first thing wrong with the steps: `{ i, field, text }` (the step's
 *  index, 'time' or 'rate', one line), or null. The first step is always
 *  at 0. `durationS` 0 is not checked yet (the duration has its own item). */
export function stepsCheck(steps, durationS) {
  if (!steps.length) return { i: -1, field: null, text: 'Add at least one step.' };
  if (steps.length > MAX_STEPS) return { i: -1, field: null, text: `At most ${MAX_STEPS} steps.` };
  let prev = -1;
  for (let i = 0; i < steps.length; i++) {
    const n = i + 1;
    const v = steps[i].value;
    if (typeof v !== 'number' || !Number.isFinite(v) || v < 0) return { i, field: 'rate', text: `Step ${n}: give a rate of 0 or more.` };
    const at = i === 0 ? 0 : stepAt(steps[i]);
    if (Number.isNaN(at)) return { i, field: 'time', text: `Step ${n}: give its start in hours and minutes.` };
    if (at <= prev) return { i, field: 'time', text: `Step ${n} starts at or before step ${n - 1}: each step starts later than the one before.` };
    if (i > 0 && durationS > 0 && at >= durationS) return { i, field: 'time', text: `Step ${n} starts after the run ends: lengthen the duration.` };
    prev = at;
  }
  return null;
}

/** `stepsCheck`'s line, or null. */
export const stepsProblem = (steps, durationS) => stepsCheck(steps, durationS)?.text ?? null;

/** The daemon's Step curve fields for valid steps. */
export function stepsCurve(steps) {
  const segments = steps.map((s, i) => ({ at_seconds: i === 0 ? 0 : stepAt(s), value: s.value }));
  return { start: segments[0].value, end: segments.at(-1).value, segments };
}

/** A past run's segments back into form steps (Run again, a settings file). */
export function stepsFromSegments(segments) {
  return [...(segments ?? [])]
    .sort((a, b) => a.at_seconds - b.at_seconds)
    .map((s) => ({ ...splitAt(Math.round(s.at_seconds)), value: s.value }));
}

/** A step's length in few words: "6 h", "6 h 30", "45 min"; '' for none. */
export function spanLabel(s) {
  if (!(s > 0)) return '';
  const m = Math.round(s / 60);
  const h = Math.floor(m / 60);
  const mm = m % 60;
  if (!h) return `${Math.max(1, mm)} min`;
  return mm ? `${h} h ${String(mm).padStart(2, '0')}` : `${h} h`;
}

/** How long step `i` lasts: to the next one, the last to the run's end. */
export function stepLasts(steps, i, durationS) {
  const a = i === 0 ? 0 : stepAt(steps[i]);
  const b = i + 1 < steps.length ? stepAt(steps[i + 1]) : durationS;
  return b - a;
}

// --- the file -----------------------------------------------------------------

/** Rows of a CSV text. Separated by ";" (decimal comma, as a French Excel
 *  saves it) or by "," (decimal point): never both readings of "1,5". */
export function readCsv(text) {
  // a byte order mark first (Excel's "CSV UTF-8") is not part of the header
  const lines = (text.charCodeAt(0) === 0xfeff ? text.slice(1) : text).split(/\r?\n/);
  const sep = lines.some((l) => l.includes(';')) ? ';' : ',';
  return lines.map((l) =>
    l.split(sep).map((c) => {
      const t = c.trim().replace(/^"(.*)"$/, '$1').trim();
      if (t === '') return null;
      const n = sep === ';' ? t.replace(',', '.') : t;
      return /^[+-]?(\d+\.?\d*|\.\d+)$/.test(n) ? Number(n) : t;
    }),
  );
}

const norm = (v) => (typeof v === 'string' ? v.trim().replace(/\s+/g, ' ').toLowerCase() : v);
const empty = (v) => v == null || (typeof v === 'string' && v.trim() === '');

/**
 * The steps in a sheet's rows (`readXlsx` or `readCsv`): `{ steps }`, or
 * `{ errors }` (one line each, row numbers as Excel shows them).
 * `unit`: the run's, 'ml/min' or 'rpm'. `max`: the highest rate the pump
 * and tube deliver, said by `maxWhy` ("this tube delivers 14.2 ml/min at
 * 350 rpm"), or null.
 */
export function parseStepRows(rows, { unit, max = null, maxWhy = '' }) {
  const head = rows[0] ?? [];
  const other = unit === 'rpm' ? 'ml/min' : 'rpm';
  if (norm(head[0]) !== norm(HEAD_TIME)) {
    return { errors: [`Row 1: column A must be headed "${HEAD_TIME}", as in the template.`] };
  }
  if (norm(head[1]) === norm(headRate(other))) {
    return { errors: [`This file is in ${other}, the run is in ${unit}. Download the ${unit} template.`] };
  }
  if (norm(head[1]) !== norm(headRate(unit))) {
    return { errors: [`Row 1: column B must be headed "${headRate(unit)}", as in the template.`] };
  }

  // the last row with anything in A or B; empty rows past it are nothing
  let last = rows.length - 1;
  while (last > 0 && empty(rows[last]?.[0]) && empty(rows[last]?.[1])) last--;
  if (last === 0) return { errors: ['The file has no steps under its headers.'] };
  if (last > MAX_STEPS) return { errors: [`${last} steps: at most ${MAX_STEPS}.`] };

  const errors = [];
  const steps = [];
  let prev = null;
  for (let r = 1; r <= last; r++) {
    const row = rows[r] ?? [];
    const n = r + 1;
    const [a, b] = row;
    if (empty(a) && empty(b)) {
      errors.push(`Row ${n} is empty.`);
      continue;
    }
    let at = null;
    if (empty(a)) errors.push(`Row ${n}: no time in column A.`);
    else if (typeof a === 'object' && 'time' in a) errors.push(`Row ${n}: column A is a time of day; give hours as a number, 12.5 for 12 h 30 min.`);
    else if (typeof a !== 'number') errors.push(`Row ${n}: "${a}" in column A is not a number of hours.`);
    else if (a < 0) errors.push(`Row ${n}: ${a} h is negative.`);
    else at = Math.round(a * 3600);

    if (empty(b)) errors.push(`Row ${n}: no rate in column B.`);
    else if (typeof b !== 'number') errors.push(`Row ${n}: "${typeof b === 'object' ? 'a date' : b}" in column B is not a rate.`);
    else if (b < 0) errors.push(`Row ${n}: ${b} ${unit} is negative.`);
    else if (max != null && b > max + 1e-9) errors.push(`Row ${n}: ${b} ${unit} is more than ${maxWhy || `the most, ${num(max, 2)} ${unit}`}.`);

    if (at != null) {
      if (r === 1 && at !== 0) errors.push(`Row ${n}: the first step must start at 0 h.`);
      else if (prev != null && at <= prev.at) errors.push(`Row ${n}: ${a} h is not after row ${prev.n}'s ${prev.h} h.`);
      prev = { at, n, h: a };
    }
    if (typeof b === 'number') steps.push({ ...splitAt(at ?? 0), value: b });
  }
  return errors.length ? { errors } : { steps };
}

/** The template workbook's sheets, for `writeXlsx`: the steps sheet, with
 *  examples and Excel's own checks on the cells, then a "Read me" sheet. */
export function templateSheets(unit) {
  const examples = unit === 'rpm'
    ? [[0, 20], [6, 40], [12.5, 60], [24, 80]]
    : [[0, 0.8], [6, 1.5], [12.5, 2.5], [24, 4]];
  const range = (col) => `${col}2:${col}${MAX_STEPS + 1}`;
  return [
    {
      name: 'Steps',
      rows: [[{ text: HEAD_TIME, bold: true }, { text: headRate(unit), bold: true }], ...examples],
      widths: [14, 16],
      freezeHeader: true,
      validations: [
        {
          type: 'decimal', operator: 'greaterThanOrEqual', formula: '0', sqref: range('A'),
          promptTitle: 'From (h)', prompt: 'Hours from the run start, as a number: 12.5 for 12 h 30 min. The first step is at 0.',
          errorTitle: 'Hours as a number', error: 'Hours as a number: 12.5 for 12 h 30 min.',
        },
        {
          type: 'decimal', operator: 'greaterThanOrEqual', formula: '0', sqref: range('B'),
          promptTitle: headRate(unit), prompt: `The rate held from this time on, in ${unit}.`,
          errorTitle: `A rate in ${unit}`, error: `A number of ${unit}, 0 or more.`,
        },
      ],
    },
    {
      name: 'Read me',
      rows: [
        [{ text: 'Fermentool, Steps feed profile', bold: true }],
        [`Column A: hours from the run start, as a number (12.5 for 12 h 30 min). The first step is at 0, each next one later.`],
        [`Column B: the rate in ${unit}, held from that time until the next step.`],
        ['After the last step the pump holds its rate until the run ends, then until Stop.'],
        [`Keep the headers of row 1 as they are, and no empty row between steps. At most ${MAX_STEPS} steps.`],
        ['Replace the example rows with yours, then import the file in New run, Feed profile, Steps.'],
      ],
      widths: [110],
    },
  ];
}
