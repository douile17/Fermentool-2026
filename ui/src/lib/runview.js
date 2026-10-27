import { LOCALE } from './fmt.js';

// Delivery tolerance: within ±2 % of what was asked, or ±1 mL while that is
// less (at the start ±2 % is a fraction of a mL, inside the balance's own
// noise). Under 50 mL asked the gap reads in mL rather than in %.
export const DELIVERY_TOL = 0.02;
export const NOISE_ML = 1;
export const MIN_ASKED_ML = 50;
export const toleranceMl = (askedMl) => Math.max(DELIVERY_TOL * askedMl, NOISE_ML);

// What the Overview of a running dosing cycle says, worked out from the
// status frame: the run's state in words, when the bottle runs dry, and the
// times shown as clock times rather than durations to add up.

/** Linear interpolation in a `[t, v][]` series, held flat past both ends. */
export function valueAt(series, t) {
  if (!series?.length) return null;
  if (t <= series[0][0]) return series[0][1];
  const last = series[series.length - 1];
  if (t >= last[0]) return last[1];
  // Bisection: the series has ~200 points and this runs per animation frame.
  let lo = 0;
  let hi = series.length - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (series[mid][0] < t) lo = mid;
    else hi = mid;
  }
  const [t0, v0] = series[lo];
  const [t1, v1] = series[hi];
  return t1 === t0 ? v0 : v0 + ((v1 - v0) * (t - t0)) / (t1 - t0);
}

/** "73 h 05", minutes always two digits so the readout keeps its width. */
export function hm(seconds) {
  const m = Math.max(0, Math.floor((seconds || 0) / 60));
  return `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, '0')}`;
}

/** "73:05:12", a clock reading of a duration: compact enough for a 100 h
 *  run, two-digit minutes and seconds so it keeps its width as it counts. */
export function clock(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  const p = (n) => String(n).padStart(2, '0');
  return `${Math.floor(s / 3600)}:${p(Math.floor(s / 60) % 60)}:${p(s % 60)}`;
}

/** A moment as people say it: "22:30" today, "Thu 04:10" on another day,
 *  "Thu 8 Oct, 13:12" with `withDate`. */
export function when(ms, nowMs = Date.now(), withDate = false) {
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return '–';
  const time = d.toLocaleTimeString(LOCALE, { hour: '2-digit', minute: '2-digit' });
  if (withDate) {
    const day = d.toLocaleDateString(LOCALE, { weekday: 'short', day: 'numeric', month: 'short' });
    return `${day}, ${time}`;
  }
  const sameDay = new Date(nowMs).toDateString() === d.toDateString();
  return sameDay ? time : `${d.toLocaleDateString(LOCALE, { weekday: 'short' })} ${time}`;
}

/** Seconds until the feed has drawn `grams` more from the bottle, or null
 *  when it cannot be told. Integrates the curve forward from `elapsedS` (it
 *  holds its end value past `durationS`, as the run does), times the
 *  correction `c`, times `gPerUnit`: grams per minute for one unit of
 *  setpoint, measured by the balance, so it holds for rpm and ml/min runs
 *  alike. Gives up past 14 days. A difference of weights only: the balance
 *  also weighs the bottle and its tubing, so "until it runs dry" from the
 *  reading alone came out too late, and is no longer shown. */
export function drawnInS({ grams, gPerUnit, planned, elapsedS, durationS, c = 1 }) {
  if (!(grams > 0) || !(gPerUnit > 0) || !planned?.length) return null;
  const step = 60;
  let left = grams;
  for (let s = 0; s < 14 * 86400; s += step) {
    const t = Math.min(elapsedS + s, durationS);
    const gPerMin = (valueAt(planned, t) ?? 0) * c * gPerUnit;
    const used = (gPerMin * step) / 60;
    if (used >= left) return s + (step * left) / used;
    left -= used;
  }
  return null;
}

/** The averaging window for `weighedFlow`: 10 min, or a 30th of a longer
 *  run. 2 min left the balance's noise in (±3.5 % on run 184); 10 min
 *  reads within ±0.7 %, and still shows a pump drifting over the run. */
export const flowWindowS = (durationS) => Math.max(600, (durationS || 0) / 30);

/** The flow the balance weighed, in the curve's own unit, for the History
 *  chart: over a trailing `windowS` window of the tracking report's
 *  `[t, required_ml, delivered_ml]` points, delivered over required times
 *  the curve at the window's middle. So an rpm run reads in rpm with no
 *  calibration at hand, and a pump that delivers 2 % short reads 2 % under
 *  the curve. Refills are in it as the daemon estimated them. */
export function weighedFlow(points, planned, windowS = 600, maxPoints = 150) {
  const out = [];
  if (!points?.length || !planned?.length) return out;
  let j = 0;
  for (let i = 0; i < points.length; i++) {
    const [t, r, d] = points[i];
    while (j + 1 < i && t - points[j + 1][0] >= windowS) j++;
    const [t0, r0, d0] = points[j];
    if (t - t0 < windowS) continue;
    const asked = r - r0;
    if (!(asked > 1e-9)) continue;
    const mid = (t + t0) / 2;
    out.push([mid, valueAt(planned, mid) * ((d - d0) / asked)]);
  }
  // A calm line: about `maxPoints`, the last one kept.
  if (out.length <= maxPoints) return out;
  const stride = Math.ceil(out.length / maxPoints);
  const thin = out.filter((_, i) => i % stride === 0);
  if (thin.at(-1) !== out.at(-1)) thin.push(out.at(-1));
  return thin;
}

/** Seconds until an automatic refill starts: the bottle down to the cycle's
 *  `low_g`, the curve integrated forward as in `drawnInS`. Null when
 *  no refill is waiting (none set up, stopped, one under way, or already
 *  under the level). */
export function refillDueInS({ cycle, weightG, ...flow }) {
  if (!cycle || cycle.halted || cycle.phase !== 'idle' || weightG == null) return null;
  const left = weightG - cycle.low_g;
  if (!(left > 0)) return null;
  return drawnInS({ grams: left, ...flow });
}

/** The run's state in one line, worst first, with what to check when it
 *  needs someone. tone: 'ok' (feeding) | 'fill' (the bottle being refilled)
 *  | 'warn' | 'bad' | 'idle'; the band takes its colour. `word` is the
 *  state in one word (the pill), `icon` the large picture beside it:
 *  'pump' | 'fill' | 'full' | 'bottle-warn' | 'alarm' | 'warn' | 'idle'. */
export function runState(status, connected, inHold) {
  const s = status ?? {};
  const c = s.trim_c != null ? `×${s.trim_c.toFixed(3)}` : 'its last value';
  const trimmed = !!s.active?.gravimetric_trim;
  const cycle = s.refill?.cycle ?? null;
  const bad = (o) => ({ tone: 'bad', word: 'Alarm', icon: 'alarm', ...o });
  const warn = (o) => ({ tone: 'warn', word: 'Check', icon: 'warn', ...o });
  if (!connected)
    return {
      tone: 'idle',
      word: 'No news',
      icon: 'idle',
      title: 'No news from the daemon',
      detail: 'What this page shows is the last state it reported.',
      check: 'If it does not come back, start Fermentool again: the run resumes where its curve is.',
    };
  if (s.stop_pending)
    return bad({
      title: 'Stop not confirmed',
      detail: 'The Stop did not reach the pump: it may still be running.',
      check: 'Fermentool sends it again as soon as the pump answers. Check the pump, stop it by hand if needed.',
    });
  // One pump switched off silences both on the shared cable: blame neither.
  if (s.serial_ok === false && s.refill)
    return bad({
      title: 'Pumps not answering',
      detail: 'The feed pump keeps its last setpoint; the curve is not followed until the pumps answer.',
      check: `Both share the cable, one switched off silences the other: check that both are on (addresses ${s.pump_address ?? '?'} and ${s.refill.address ?? '?'}), then Connect.`,
    });
  if (s.serial_ok === false)
    return bad({
      title: 'Pump not answering',
      detail: 'The pump keeps its last setpoint; the curve is not followed until it answers.',
      check: `Check its power and its cable (address ${s.pump_address ?? '?'}), then Connect.`,
    });
  if (cycle?.phase === 'stopping')
    return bad({
      title: 'Refill pump did not stop',
      detail: 'The refill pump did not acknowledge Stop: it may still be filling the bottle.',
      check: 'Stop the refill pump by hand. Fermentool keeps sending Stop.',
    });
  if (trimmed && s.tracking?.wrong_side)
    return bad({
      title: 'Balance on the other side',
      detail: 'The weight moves the wrong way for where Settings says the balance is.',
      check: 'This run keeps its pump setpoint, uncorrected. Fix "Balance weighs" in Settings for the next run.',
    });
  if (trimmed && s.tracking?.alarm === 'feed_stopped')
    return bad({
      title: 'Feed stopped',
      detail: 'The pump turns, but the bottle weight has not moved for 3 minutes.',
      check: 'Bottle empty? Line pinched, unplugged, or full of air? Tube seated in the head?',
      note: `Correction held at ${c}. Feed missed meanwhile is not caught up.`,
    });
  if (trimmed && (s.tracking?.alarm === 'saturated' || (s.scale_connected !== false && s.scale_ok === false)))
    return bad({
      title: 'Correction out of bounds',
      detail: 'For 5 minutes the pump has delivered further from its setpoint than the correction limit can make up.',
      check: 'Check the tube in the head, leaks, the bottle. A wider limit (Settings, Balance) also lifts it.',
      note: `Correction held at ${c}; it resumes by itself once the pump is back within bounds.`,
    });
  if (trimmed && s.scale_connected === false)
    return warn({
      title: 'Balance not answering',
      detail: 'The pump keeps following the curve, without correction.',
      check: 'Check the balance cable; Fermentool keeps retrying.',
    });
  if (cycle?.halted)
    return {
      tone: 'warn',
      word: 'Refill stopped',
      icon: 'bottle-warn',
      title: 'The bottle is not being refilled',
      detail: 'Automatic refill is stopped, by an alarm or by Stop refill. The feed goes on.',
      check: 'Reservoir empty, line off or pinched, refill pump not turning? Then Refill now starts it again.',
    };
  if (s.pump_confirmed === false)
    return warn({
      title: 'Pump not tracking the setpoint',
      detail: 'Writes get through but the pump reports a different value.',
      check: 'Check the pump.',
    });
  if (s.journal_ok === false)
    return warn({
      title: 'Journal stalled',
      detail: 'The pump runs correctly, but the run history is not being saved.',
      check: 'Check free disk space.',
    });
  if (cycle?.phase === 'filling' && cycle.not_rising)
    return {
      tone: 'warn',
      word: 'Not filling',
      icon: 'bottle-warn',
      title: 'The bottle is not filling',
      detail: 'The refill pump keeps going, in case the line is only priming.',
      check: 'Reservoir empty? Line off, pinched or not primed? Refill pump turning? Stop refill if it cannot work.',
    };
  if (cycle?.phase === 'filling')
    return {
      tone: 'fill',
      word: 'Refilling',
      icon: 'fill',
      title: 'Refill pump filling the bottle',
      detail: 'The feed goes on; the correction is held until the weight is still.',
      progress: refillProgress(cycle),
    };
  if (cycle?.phase === 'settling' || (trimmed && s.scale_state === 'refill_settling'))
    return {
      tone: 'fill',
      word: 'Settling',
      icon: 'full',
      title: 'Bottle full, waiting for the weight to settle',
      detail: 'The correction resumes once the weight holds still.',
    };
  if (trimmed && s.scale_state === 'refill_pending')
    return {
      tone: 'fill',
      word: 'Refilling',
      icon: 'fill',
      title: 'Refilling the bottle',
      detail: 'Correction held while the bottle is refilled; it resumes once the weight is still.',
    };
  if (inHold)
    return {
      tone: 'ok',
      word: 'Holding',
      icon: 'pump',
      title: 'Curve done, holding its end value',
      detail: 'Still regulated and recorded. Stopping now records the run as completed.',
    };
  return { tone: 'ok', word: 'Feeding', icon: 'pump', title: 'Running normally' };
}

/** How far a refill pump's fill has got, from the refill level `low_g` to
 *  the full bottle `high_g`: `{ weightG, toG, frac, riseGMin, leftMin }`,
 *  or null when nothing is filling. The time left comes from how fast the
 *  balance sees the bottle rise (net of the feed), not from an expected
 *  flow: the refill pump is not calibrated. */
export function refillProgress(cycle) {
  if (cycle?.phase !== 'filling' || cycle.weight_g == null) return null;
  const span = cycle.high_g - cycle.low_g;
  const frac = span > 0 ? Math.max(0, Math.min(1, (cycle.weight_g - cycle.low_g) / span)) : 0;
  const left = Math.max(0, cycle.high_g - cycle.weight_g);
  const rise = cycle.rise_g_per_min ?? null;
  return {
    weightG: cycle.weight_g,
    toG: cycle.high_g,
    frac,
    riseGMin: rise,
    leftMin: rise > 0 ? left / rise : null,
  };
}

/** The window's title for a run in state `rs`: the state first, so it
 *  reads in the taskbar or among other tabs ("Refilling 42 % · Fermentool").
 *  `value` is what the pump is set to, already formatted with its unit. */
export function stateTitle(rs, value = null) {
  if (!rs || rs.tone === 'idle') return 'Fermentool';
  let what = rs.title;
  if (rs.tone === 'ok') what = value ? `${rs.word} ${value}` : rs.word;
  else if (rs.progress) what = `${rs.word} ${Math.round(rs.progress.frac * 100)} %`;
  else if (rs.tone === 'fill') what = rs.word;
  return `${what} · Fermentool`;
}

/** "in about 35 min" under an hour, "in 2 h 05" past it. */
export function inAbout(seconds) {
  const s = Math.max(0, seconds || 0);
  return s < 3600 ? `in about ${Math.max(1, Math.round(s / 60))} min` : `in ${hm(s)}`;
}

/** The band's "Next refill" for a run with automatic refill, idle between
 *  fills: when the bottle reaches the refill level (`inS`, null when the
 *  balance cannot tell yet) and the levels it refills between. Null while a
 *  fill is under way or the cycle is stopped: the band says so itself. */
export function nextRefill({ cycle, inS, nowMs }) {
  if (!cycle || cycle.halted || cycle.phase !== 'idle') return null;
  const levels = `at ${kgOf(cycle.low_g)}, back to ${kgOf(cycle.high_g)}`;
  if (inS == null) return { label: 'Next refill', what: `under ${kgOf(cycle.low_g)}`, sub: `back to ${kgOf(cycle.high_g)}` };
  return { label: 'Next refill', what: `around ${when(nowMs + inS * 1000, nowMs)}`, sub: `${inAbout(inS)}, ${levels}` };
}
const kgOf = (g) => `${(g / 1000).toFixed(2)} kg`;

/** The band of a run holding past its curve (mockup C, 2026-10-22): the
 *  automatic refill goes on, so its time is what the band says next, and
 *  Stop moves into the line under the title; with none waiting, the band
 *  asks for Stop. `{ next, detail }`, `detail` null to keep runState's. */
export function holdBand({ cycle, inS, nowMs, holdS }) {
  const nr = nextRefill({ cycle, inS, nowMs });
  if (nr) return { next: nr, detail: `Done ${hm(holdS)} ago, the pump still feeds. Stop records the run as completed.` };
  return { next: { what: 'Stop the run when you are done', sub: `curve done ${hm(holdS)} ago, pump still feeding` }, detail: null };
}

/** The chart's refill mark: at its time inside the curve's span, or on the
 *  right edge with an arrow when it falls past the end (a run holding past
 *  its curve). `{ t, label }` for the chart, null for none. */
export function refillMarkAt(refill, spanS) {
  if (!refill || !(refill.t > 0) || !(spanS > 0)) return null;
  if (refill.t < spanS) return { t: refill.t, label: refill.label, edge: false };
  return { t: spanS, label: `${refill.label} →`, edge: true };
}

/** The Delivery panel's figures from the last tracking point
 *  `[t, asked_ml, weighed_ml]`: the share delivered and the gap, both in %.
 *  "0.0 %" alone read as nothing delivered; "100.0 % delivered" cannot. */
export function deliveryFigures(point) {
  if (!point || !(point[1] > 0)) return null;
  const pct = (100 * point[2]) / point[1];
  return { deliveredPct: pct, gapPct: pct - 100 };
}
