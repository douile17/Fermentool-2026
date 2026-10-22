// The rate of a Constant feed run, changed while it runs (2026-10-22): its
// steps, the words the Overview, History and Run again put on them, and the
// check of a new rate before it is sent. Plain functions of the run as
// `GET /api/runs/:id` gives it (`setpoint_changes`) and of the status frame.

import { LOCALE, dur, num, unitFor, digitsFor } from './fmt.js';
import { atRun } from './rundetail.js';

/** The rates a run went through: the start rate (at `started_at`), then
 *  each change, `[{ value, wallMs, elapsedS, atStart, last }]`. One step
 *  when the rate never changed. */
export function rateSteps(run) {
  const steps = [{ value: run.curve.start, wallMs: Date.parse(run.started_at), elapsedS: 0, atStart: true }];
  for (const c of run.setpoint_changes ?? []) {
    steps.push({ value: c.value, wallMs: Date.parse(c.wall_time), elapsedS: c.elapsed_s, atStart: false });
  }
  return steps.map((s, i) => ({ ...s, last: i === steps.length - 1 }));
}

/** The day and time of a step, "Mon 20:30": Run again speaks of real moments
 *  of the week, the charts of run time. */
export function rateLabel(step) {
  const d = new Date(step.wallMs);
  if (Number.isNaN(d.getTime())) return '–';
  const day = d.toLocaleDateString(LOCALE, { weekday: 'short' });
  return `${day} ${d.toLocaleTimeString(LOCALE, { hour: '2-digit', minute: '2-digit' })}`;
}

/** Since when the rate of a Constant run holds, under the Overview's feed
 *  rate: "constant since the start", "since Mon 20:30". */
export function rateSince(run) {
  const steps = rateSteps(run);
  return steps.length === 1 ? 'constant since the start' : `since ${rateLabel(steps.at(-1))}`;
}

/** The Overview's "Curve" fact: "Constant, 24 h" (its rate, which may have
 *  changed, is said above it), "5.0 → 50.0" for any other curve. */
export function curveFact(run) {
  if (run.curve?.params?.kind === 'constant') return `Constant, ${dur(run.duration_s).replace(/ 0 [ms]$/, '')}`;
  const d = digitsFor(run.control_var);
  return `${num(run.curve.start, d)} → ${num(run.curve.end, d)}`;
}

/** Whether the Overview offers "Change rate": the active feed run, a
 *  Constant curve, one the daemon gives bounds for. */
export function canChangeRate(status, run) {
  const a = status?.active;
  if (!a || !run || a.kind === 'calibration' || a.run_id !== run.id) return false;
  return run.curve?.params?.kind === 'constant' && Array.isArray(a.rate_bounds);
}

/** Why a new rate cannot be sent, shown under the field, or null.
 *  `bounds` is the status frame's `rate_bounds` (`[min, max]`), `current`
 *  the rate now: the same rate, as shown, is no change. */
export function checkRate(value, bounds, current, controlVar = 'ml_min') {
  const u = unitFor(controlVar);
  const d = digitsFor(controlVar);
  if (value == null || value === '' || !Number.isFinite(Number(value))) return 'Enter the new rate.';
  const v = Number(value);
  if (v <= 0) return 'The rate must be more than 0.';
  if (bounds) {
    const [lo, hi] = bounds;
    if (v < lo) return `At least ${num(lo, d)} ${u} on this run.`;
    if (v > hi) return `At most ${num(hi, d)} ${u} on this run.`;
  }
  if (current != null && num(v, d) === num(current, d)) return `The rate is already ${num(current, d)} ${u}.`;
  return null;
}

/** What the rate may be set to, under the field. */
export function boundsLine(bounds, controlVar) {
  if (!bounds) return '';
  const u = unitFor(controlVar);
  const d = digitsFor(controlVar);
  const [lo, hi] = bounds;
  return lo > 0 ? `${num(lo, d)} to ${num(hi, d)} ${u} on this run` : `More than 0, at most ${num(hi, d)} ${u} on this run`;
}

/** The changes as the History chart's numbered markers, `[{ t, text }]`,
 *  the text said under the chart after its number. */
export function rateMarks(run) {
  const u = unitFor(run.control_var);
  const d = digitsFor(run.control_var);
  return (run.setpoint_changes ?? []).map((c, i) => ({
    t: c.elapsed_s,
    text: `at ${atRun(c.elapsed_s)}, ${i === 0 ? 'rate changed ' : ''}to ${num(c.value, d)} ${u}`,
  }));
}

/** Run again from one of the rates: New run's seed with that constant. */
export const runAgainSeed = (seed, value) => ({ ...seed, kind: 'constant', start: value, end: value, value });
