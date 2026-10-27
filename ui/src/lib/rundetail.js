// A past run's detail (History): what it says first, the verdict, and the
// words it puts on the bottle, the regulation and the journal. Plain
// functions of the run, its tracking report and its events.

import { LOCALE, num, dur, unitFor, digitsFor } from './fmt.js';
import { inSpans } from './refill.js';
import { niceStep } from './chartscale.js';
import { modelOf } from './pumps.js';

/** The ±share of requested a delivered total may differ by and count as on target. */
export const TOLERANCE = 0.02;
/** The first minutes are left out of the worst gap: the correction is still
 *  being measured, and a gram over a few mL reads as tens of %. */
export const SETTLE_S = 900;

const pad = (n) => String(n).padStart(2, '0');
const hm = (d) => `${pad(d.getHours())}:${pad(d.getMinutes())}`;
const dayMonth = (d) => d.toLocaleDateString(LOCALE, { weekday: 'short', day: 'numeric', month: 'short' });
const sameDay = (a, b) => a.toDateString() === b.toDateString();

/** Thousands apart with a thin space: "3 305.6". */
export function grouped(x, digits = 1) {
  if (x == null || Number.isNaN(x)) return '–';
  const [int, frac] = Math.abs(x).toFixed(digits).split('.');
  const g = int.replace(/\B(?=(\d{3})+(?!\d))/g, ' ');
  return `${x < 0 ? '−' : ''}${g}${frac ? `.${frac}` : ''}`;
}

/** "Mon 19 Oct 17:15 → Tue 20 Oct 09:00", the end's date only when it is
 *  another day; "Mon 19 Oct 17:15" alone for a run with no end. */
export function whenLine(startedIso, endedIso) {
  const a = new Date(startedIso);
  if (Number.isNaN(a.getTime())) return '–';
  const from = `${dayMonth(a)} ${hm(a)}`;
  if (!endedIso) return from;
  const b = new Date(endedIso);
  if (Number.isNaN(b.getTime())) return from;
  return `${from} → ${sameDay(a, b) ? '' : `${dayMonth(b)} `}${hm(b)}`;
}

/** Number of alarms raised in a run's events (not their clearing or ack). */
export const alarmCount = (events) =>
  (events ?? []).filter((e) => e.kind.startsWith('alarm_') && e.kind !== 'alarm_cleared' && e.kind !== 'alarm_ack').length;

/** How a weighed run went, from its tracking report: the totals, the share
 *  delivered, and the largest gap once settled, as a share of requested.
 *  null without a report (a run without the balance correction). */
export function verdict(report) {
  const pts = report?.points ?? [];
  const last = pts.at(-1);
  if (!last) return null;
  const [, requested, delivered] = last;
  const wrongSide = requested > 2 && delivered < -0.25 * requested;
  const pct = requested > 0 ? (100 * delivered) / requested : null;
  const onTarget = pct != null && Math.abs(delivered - requested) <= TOLERANCE * requested;
  // worst gap after the start-up, weighed points only
  let worst = null;
  for (const [t, r, d] of pts) {
    if (t < SETTLE_S || r <= 0 || inSpans(t, report.estimated)) continue;
    const share = Math.abs(d - r) / r;
    if (worst == null || share > worst) worst = share;
  }
  return { requested, delivered, gap: delivered - requested, pct, onTarget, wrongSide, worstPct: worst == null ? null : 100 * worst };
}

/** The 5 min average of the cumulative gap, delivered − requested (mL), for
 *  the gap chart: `[[t, gap, est]]`. The balance's 0.1 g steps made the raw
 *  line zigzag ±0.1 mL (a median still flips on such an alternation); the
 *  average keeps the trend. An estimated stretch (a refill) is not weighed:
 *  it is drawn straight from the weighed point before it to the one after
 *  (its raw estimate zigzagged into a scribble once dashed), flagged. */
export function gapSeries(points, spans, windowS = 300) {
  const raw = (points ?? []).map(([t, r, d]) => [t, d - r, inSpans(t, spans)]);
  const weighed = raw.filter((p) => !p[2]);
  const out = [];
  let lo = 0;
  let hi = 0;
  for (const [t, g, est] of raw) {
    if (est) {
      out.push([t, g, true]);
      continue;
    }
    while (lo < weighed.length && weighed[lo][0] < t - windowS / 2) lo++;
    while (hi < weighed.length && weighed[hi][0] <= t + windowS / 2) hi++;
    const win = weighed.slice(lo, hi);
    out.push([t, win.length ? win.reduce((a, p) => a + p[1], 0) / win.length : g, false]);
  }
  // each estimated run: a straight line between its weighed neighbours
  for (let i = 0; i < out.length; ) {
    if (!out[i][2]) { i++; continue; }
    let j = i;
    while (j < out.length && out[j][2]) j++;
    const before = out[i - 1];
    const after = out[j];
    for (let k = i; k < j; k++) {
      const t = out[k][0];
      if (before && after) out[k][1] = before[1] + ((after[1] - before[1]) * (t - before[0])) / (after[0] - before[0] || 1);
      else if (before) out[k][1] = before[1];
      else if (after) out[k][1] = after[1];
    }
    i = j;
  }
  return out;
}

/** A scale symmetric around 0 for the gap chart: `{ hi, ticks }`, ticks from
 *  −hi to +hi on a round step, at least ±0.5 mL so a still line stays still. */
export function gapScale(series) {
  const m = Math.max(0.5, ...(series ?? []).map((p) => Math.abs(p[1])).filter(Number.isFinite));
  const step = niceStep((m * 1.05) / 2);
  const hi = Math.ceil((m * 1.05) / step - 1e-9) * step;
  const ticks = [];
  for (let v = -hi; v <= hi + step * 1e-6; v += step) ticks.push(+v.toPrecision(12));
  return { hi, ticks };
}

/** The bottle's rows for the Feed bottle table: each refill with when it
 *  started (its estimated stretch) and how long it took. `bottle` is
 *  `bottleWeights(report)`. */
export function refillRows(bottle, spans) {
  return (bottle?.refills ?? []).map((r) => {
    const span = (spans ?? [])
      .filter((s) => Math.abs(s[1] - r.t_s) <= 30)
      .sort((a, b) => Math.abs(a[1] - r.t_s) - Math.abs(b[1] - r.t_s))[0];
    return { ...r, from_s: span ? span[0] : r.t_s, took_s: span ? span[1] - span[0] : null };
  });
}

/** Run time `t_s` as the experiment counts it, never the time of day
 *  (2026-10-22): "0", "47 min", "4 h 45", "26 h 03". */
export function atRun(t_s) {
  const m = Math.max(0, Math.floor((t_s || 0) / 60));
  if (m === 0) return '0';
  if (m < 60) return `${m} min`;
  return `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, '0')}`;
}

/** Minutes, rounded, "27 min"; seconds under a minute. */
export const mins = (s) => (s < 60 ? `${Math.round(s)} s` : `${Math.round(s / 60)} min`);

/** The feed pump a run was driven on, for its Settings: "LabQ", "LabV",
 *  "PDS II, head 2" (a run made before the models was a LabQ). */
export function pumpLine(run) {
  const m = modelOf(run.pump_model);
  return m.id === 'pds2' ? `${m.label}, head ${run.pump_head ?? 1}` : m.label;
}

/** The curve in one line: "Step, 3.500 ml/min for 15 h 44 m, then 0.000",
 *  "Linear, 13.000 → 10.000 ml/min over 1 h 0 m". */
export function curveLine(run) {
  const c = run.curve;
  const u = unitFor(run.control_var);
  const d = digitsFor(run.control_var);
  const over = dur(run.duration_s);
  const kind = c.params.kind;
  const name = kind.charAt(0).toUpperCase() + kind.slice(1);
  if (kind === 'constant') return `Constant, ${num(c.start, d)} ${u} for ${over}`;
  if (kind === 'step') {
    const segs = (c.params.segments ?? []).filter((s) => s.at_seconds < run.duration_s);
    if (segs.length <= 1) {
      const v = segs[0]?.value ?? c.start;
      return `Step, ${num(v, d)} ${u} for ${over}${Math.abs(c.end - v) > 1e-9 ? `, then ${num(c.end, d)}` : ''}`;
    }
    return `Step, ${segs.length} levels, ${num(c.start, d)} → ${num(c.end, d)} ${u} over ${over}`;
  }
  const mu = kind === 'exponential' && c.mode === 'physio' && c.params.mu_per_hour != null ? `, µ ${num(c.params.mu_per_hour, 3)} h⁻¹` : '';
  return `${name}, ${num(c.start, d)} → ${num(c.end, d)} ${u} over ${over}${mu}`;
}

/** The curve as the run detail's header chips (mockup C, 2026-10-22: its
 *  start and end were hard to find, behind Settings): `[{ text }` or
 *  `{ value, unit }]`, the shape, its values, its length, the direction. */
export function curveChips(run) {
  const c = run.curve;
  const u = unitFor(run.control_var);
  const d = digitsFor(run.control_var);
  const kind = c.params.kind;
  const out = [{ text: kind.charAt(0).toUpperCase() + kind.slice(1) }];
  const changes = run.setpoint_changes ?? [];
  if (kind === 'constant' && changes.length) {
    // changed while it ran: each rate, or past a few, how many and the last
    const then = changes.length <= 3
      ? `then ${changes.map((s) => num(s.value, d)).join(', ')}`
      : `changed ${changes.length}×, last ${num(changes.at(-1).value, d)}`;
    out.push({ value: num(c.start, d), unit: `${u}, ${then}` });
  } else if (kind === 'constant') out.push({ value: num(c.start, d), unit: u });
  else if (kind === 'step') {
    const segs = (c.params.segments ?? []).filter((s) => s.at_seconds < run.duration_s);
    if (segs.length <= 1) {
      const v = segs[0]?.value ?? c.start;
      out.push({ value: num(v, d), unit: Math.abs(c.end - v) > 1e-9 ? `${u}, then ${num(c.end, d)}` : u });
    } else out.push({ value: `${num(c.start, d)} → ${num(c.end, d)}`, unit: `${u}, ${segs.length} levels` });
  } else out.push({ value: `${num(c.start, d)} → ${num(c.end, d)}`, unit: u });
  if (kind === 'exponential' && c.mode === 'physio' && c.params.mu_per_hour != null) out.push({ text: `µ ${num(c.params.mu_per_hour, 3)} h⁻¹` });
  out.push({ text: `${kind === 'constant' || kind === 'step' ? 'for' : 'over'} ${dur(run.duration_s)}` });
  out.push({ text: run.direction === 'ccw' ? 'counter-clockwise' : 'clockwise' });
  return out;
}

// --- the journal, in words ------------------------------------------------

const TITLES = {
  start: 'Started',
  resume: 'Resumed',
  auto_resume: 'Resumed by itself',
  crash_detected: 'Daemon restarted',
  stop: 'Stopped',
  curve_done: 'Curve done',
  trim_start: 'Balance correction on',
  trim_ratio: 'Pump ratio measured',
  trim_limit: 'Correction limit changed',
  alarm_feed_stopped: 'Alarm: feed stopped',
  alarm_saturated: 'Alarm: correction out of bounds',
  alarm_wrong_side: 'Alarm: balance on the other side',
  alarm_refill_no_rise: 'Alarm: bottle not filling',
  alarm_refill_overfill: 'Alarm: bottle overfilled',
  alarm_refill_stop_failed: 'Alarm: refill pump did not stop',
  alarm_cleared: 'Alarm cleared',
  alarm_ack: 'Alarm acknowledged',
  scale_lost: 'Balance lost',
  scale_recovered: 'Balance back',
  serial_lost: 'Pump link lost',
  serial_recovered: 'Pump link back',
  refill_cancelled: 'Refill cancelled',
  refill_interrupted: 'Refill interrupted',
  refill_pump_lost: 'Refill pump lost',
  refill_pump_ok: 'Refill pump back',
  setpoint_change: 'Rate changed',
};
const kindTitle = (k) => TITLES[k] ?? (k.charAt(0).toUpperCase() + k.slice(1)).replaceAll('_', ' ');

/** A run's events as the journal's lines, in time order: `[{ day, time,
 *  title, detail, tone }]`. A refill's start, stop and settle make one line
 *  ("Refill 2", how long, at what speed, the weights); the correction's
 *  start and its first ratio, a minute apart, one too. `time` is the
 *  experiment's time since the start (`atRun`), not the time of day.
 *  `tone`: 'refill', 'bad' for an alarm or an error, '' otherwise. */
export function journalLines(events, run) {
  // Same-second events (the correction and its first ratio) keep the order
  // they were journalled in, by id.
  const evs = [...(events ?? [])].sort((a, b) => Date.parse(a.wall_time) - Date.parse(b.wall_time) || (a.id ?? 0) - (b.id ?? 0));
  const u = unitFor(run.control_var);
  const out = [];
  let refillN = 0;
  let openRefill = null; // { line, rpm, before }
  let lastRefill = null; // the one last closed, in case it resumes
  // the run's start; with none known, its first event
  const startMs = Number.isFinite(Date.parse(run.started_at)) ? Date.parse(run.started_at) : Date.parse(evs[0]?.wall_time);
  const push = (iso, title, detail, tone = '') => {
    const ms = Date.parse(iso);
    const line = { time: atRun((ms - startMs) / 1000), title, detail: detail ?? '', tone, ms };
    out.push(line);
    return line;
  };
  for (const e of evs) {
    const det = e.detail ?? '';
    const tone = e.level === 'error' || e.level === 'warn' || (e.kind.startsWith('alarm_') && e.kind !== 'alarm_cleared' && e.kind !== 'alarm_ack') ? 'bad' : '';
    if (e.kind === 'refill_start') {
      // a fill cut by a silent balance and resumed is the same refill: its
      // line goes on (run 210 showed "Refill 1" and "Refill 2" for one)
      if (/^resuming/.test(det) && lastRefill) {
        openRefill = lastRefill;
        continue;
      }
      refillN += 1;
      const rpm = det.match(/at ([\d.]+) rpm/)?.[1];
      openRefill = { line: push(e.wall_time, `Refill ${refillN}`, rpm ? `at ${rpm} rpm` : '', 'refill'), rpm };
      continue;
    }
    if (e.kind === 'refill_stop' || e.kind === 'refill_rising' || e.kind === 'refill_restarted') continue;
    if (e.kind === 'refill') {
      const w = det.match(/([\d.]+) g -> ([\d.]+) g/);
      const weights = w ? `${grouped(+w[1])} → ${grouped(+w[2])} g` : det;
      if (openRefill) {
        const took = (Date.parse(e.wall_time) - openRefill.line.ms) / 1000;
        // resumed: from the first weight before it to the last one after
        openRefill.before ??= w?.[1];
        const span = w && openRefill.before != null ? `${grouped(+openRefill.before)} → ${grouped(+w[2])} g` : weights;
        openRefill.line.detail = [mins(took) + (openRefill.rpm ? ` at ${openRefill.rpm} rpm` : ''), span].join(', ');
        lastRefill = openRefill;
        openRefill = null;
      } else {
        refillN += 1;
        push(e.wall_time, `Refill ${refillN}`, `by hand, ${weights}`, 'refill');
      }
      continue;
    }
    if (e.kind === 'trim_ratio') {
      const prev = out.at(-1);
      const ratio = det.match(/([\d.]+)/)?.[1];
      if (prev?.title === TITLES.trim_start && Date.parse(e.wall_time) - prev.ms <= 60_000 && ratio) {
        prev.detail = `pump ratio ${ratio}`;
        continue;
      }
      push(e.wall_time, TITLES.trim_ratio, ratio ?? det, tone);
      continue;
    }
    if (e.kind === 'trim_start') {
      const c = det.match(/x([\d.]+)/)?.[1];
      push(e.wall_time, TITLES.trim_start, c ? `correction ×${c}` : det, tone);
      continue;
    }
    if (e.kind === 'start') {
      const v = det.match(/([\d.]+)/)?.[1];
      push(e.wall_time, TITLES.start, v ? `at ${num(+v, digitsFor(run.control_var))} ${u}` : det, tone);
      continue;
    }
    if (e.kind === 'curve_done') {
      const v = det.match(/holding at ([\d.]+)/)?.[1];
      push(e.wall_time, TITLES.curve_done, v ? `holding ${v} ${u} until Stop` : det, tone);
      continue;
    }
    if (e.kind === 'setpoint_change') {
      push(e.wall_time, TITLES.setpoint_change, det.replace(' -> ', ' → '), tone);
      continue;
    }
    if (e.kind === 'stop') {
      push(e.wall_time, TITLES.stop, det || run.status, tone);
      continue;
    }
    push(e.wall_time, kindTitle(e.kind), det, tone);
  }
  return out;
}

/** The regulation's two figures from the journal: the correction it started
 *  at and the pump ratio it first measured (strings as journalled), each
 *  null when absent. */
export function regulationFigures(events) {
  const evs = events ?? [];
  const c = evs.find((e) => e.kind === 'trim_start')?.detail?.match(/x([\d.]+)/)?.[1] ?? null;
  const k = evs.find((e) => e.kind === 'trim_ratio')?.detail?.match(/([\d.]+)/)?.[1] ?? null;
  return { correction: c, ratio: k };
}

/** The largest weighed gap ahead and behind (mL, raw, refills left out). */
export function gapExtremes(points, spans) {
  let ahead = 0;
  let behind = 0;
  for (const [t, r, d] of points ?? []) {
    if (inSpans(t, spans)) continue;
    ahead = Math.max(ahead, d - r);
    behind = Math.min(behind, d - r);
  }
  return { ahead, behind };
}

/** The legend of the unweighed stretches on the run detail's charts: they
 *  are refills, but also bumps of a second or two (the bottle touched),
 *  which "Refill" alone called refills (runs 181, 192, 2026-10-22). */
export function blindLegend(nSpans, nRefills) {
  if (!nRefills) return 'Not weighed';
  return nSpans > nRefills ? 'Refill or bump, not weighed' : 'Refill, not weighed';
}
