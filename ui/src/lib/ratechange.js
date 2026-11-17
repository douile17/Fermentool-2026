// The rate of a Constant feed run, changed while it runs (2026-10-22): its
// steps, the words the Overview, History and Run again put on them, and the
// check of a new rate before it is sent. Plain functions of the run as
// `GET /api/runs/:id` gives it (`setpoint_changes`) and of the status frame.

import { LOCALE, dur, num, unitFor, digitsFor } from './fmt.js';
import { atRun } from './rundetail.js';

const isConstant = (run) => run.curve?.params?.kind === 'constant';

/** The rates a run went through: the start rate (at `started_at`), then
 *  each change, `[{ value, wallMs, elapsedS, atStart, last }]`. One step
 *  when the rate never changed. Any other curve than Constant starts at its
 *  end value, from the end of the curve (`duration_s`, not `atStart`):
 *  `end`, the value the daemon drives there when given (the status frame's
 *  `curve_value`), else `run.curve.end` (a Physio curve's is not resolved). */
export function rateSteps(run, end) {
  // a curve that is not Constant is a rate only once it is done: the rate it
  // holds from its end, until a change
  const first = isConstant(run)
    ? { value: run.curve.start, wallMs: Date.parse(run.started_at), elapsedS: 0, atStart: true }
    : { value: end ?? run.curve.end, wallMs: Date.parse(run.started_at) + run.duration_s * 1000, elapsedS: run.duration_s, atStart: false };
  const steps = [first];
  for (const c of run.setpoint_changes ?? []) {
    steps.push({ value: c.value, wallMs: Date.parse(c.wall_time), elapsedS: c.elapsed_s, atStart: false });
  }
  return steps.map((s, i) => ({ ...s, last: i === steps.length - 1 }));
}

/** The day and time of a step, "Mon 20:30", and for a change how far into
 *  the run it came, "Mon 20:30, at 6 h 20": the moment of the week, and the
 *  experiment's time the charts count in. */
export function rateLabel(step) {
  const d = new Date(step.wallMs);
  if (Number.isNaN(d.getTime())) return '–';
  const day = d.toLocaleDateString(LOCALE, { weekday: 'short' });
  const when = `${day} ${d.toLocaleTimeString(LOCALE, { hour: '2-digit', minute: '2-digit' })}`;
  return step.atStart ? when : `${when}, at ${atRun(step.elapsedS)}`;
}

/** Since when the rate of a Constant run holds, under the Overview's feed
 *  rate: "constant since the start", "since Mon 20:30, at 6 h 20". */
export function rateSince(run, curveDone = false) {
  const changes = run.setpoint_changes ?? [];
  // a hold is told in run time, like the charts
  if (!isConstant(run) && changes.length) return `holding since ${atRun(changes.at(-1).elapsed_s)}`;
  const steps = rateSteps(run);
  return steps.length === 1 ? (curveDone && !isConstant(run) ? 'holding its end value' : 'constant since the start') : `since ${rateLabel(steps.at(-1))}`;
}

/** The Change rate dialog's steps and rate now, `{ steps, current }`, from
 *  the status frame's `curveValue` (what the daemon drives, before the
 *  balance correction); the steps' last value without it (an older daemon). */
export function changeRateNow(run, curveValue) {
  const known = curveValue != null && Number.isFinite(curveValue);
  // the curve's end is the rate now only until a change
  const unchanged = !(run.setpoint_changes ?? []).length;
  const steps = rateSteps(run, known && unchanged ? curveValue : undefined);
  return { steps, current: known ? curveValue : steps.at(-1).value };
}

/** The ChangeRate dialog's title and the scope line under it. */
export function rateDialogText(run) {
  return isConstant(run)
    ? { title: 'New constant rate', scope: 'From now until the end of the run, or the next change.' }
    : { title: 'New hold rate', scope: 'From now until you stop the run, or the next change.' };
}

/** The Overview's "hold" chip for a non-constant run whose hold was changed:
 *  `{ value, unit }`, null when nothing was changed or the curve is Constant
 *  (its chip says the changes itself). */
export function holdChip(run) {
  const last = (run.setpoint_changes ?? []).at(-1);
  if (isConstant(run) || !last) return null;
  return { value: num(last.value, digitsFor(run.control_var)), unit: unitFor(run.control_var) };
}

/** Whether Run again offers the run's rates to pick from: a Constant run
 *  that changed. A hold change on another curve is not part of its shape. */
export const runAgainPicks = (run) => isConstant(run) && (run.setpoint_changes ?? []).length > 0;

/** The Overview's "Curve" fact: "Constant, 24 h" (its rate, which may have
 *  changed, is said above it), "4 steps, 1.000 to 6.000" (start → end hid
 *  the steps between), "5.0 → 50.0" for any other curve. */
export function curveFact(run) {
  if (run.curve?.params?.kind === 'constant') return `Constant, ${dur(run.duration_s).replace(/ 0 [ms]$/, '')}`;
  const d = digitsFor(run.control_var);
  const segs = run.curve?.params?.kind === 'step' ? (run.curve.params.segments ?? []).filter((s) => s.at_seconds < run.duration_s) : [];
  if (segs.length > 1) {
    const vs = segs.map((s) => s.value);
    return `${segs.length} steps, ${num(Math.min(...vs), d)} to ${num(Math.max(...vs), d)}`;
  }
  return `${num(run.curve.start, d)} → ${num(run.curve.end, d)}`;
}

/** Whether the Overview offers "Change rate": the active feed run, one the
 *  daemon gives bounds for (a Constant curve, or any curve once it is done). */
export function canChangeRate(status, run) {
  const a = status?.active;
  if (!a || !run || a.kind === 'calibration' || a.run_id !== run.id) return false;
  return Array.isArray(a.rate_bounds);
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
