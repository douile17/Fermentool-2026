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
  const time = d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  if (withDate) {
    const day = d.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
    return `${day}, ${time}`;
  }
  const sameDay = new Date(nowMs).toDateString() === d.toDateString();
  return sameDay ? time : `${d.toLocaleDateString(undefined, { weekday: 'short' })} ${time}`;
}

/** Seconds until a feed bottle of `weightG` runs dry, or null when it cannot
 *  be told. Integrates the curve forward from `elapsedS` (it holds its end
 *  value past `durationS`, as the run does), times the correction `c`, times
 *  `gPerUnit`: grams per minute for one unit of setpoint, measured by the
 *  balance, so it holds for rpm and ml/min runs alike. Gives up past 14 days. */
export function bottleEmptyInS({ weightG, gPerUnit, planned, elapsedS, durationS, c = 1 }) {
  if (!(weightG > 0) || !(gPerUnit > 0) || !planned?.length) return null;
  const step = 60;
  let left = weightG;
  for (let s = 0; s < 14 * 86400; s += step) {
    const t = Math.min(elapsedS + s, durationS);
    const gPerMin = (valueAt(planned, t) ?? 0) * c * gPerUnit;
    const used = (gPerMin * step) / 60;
    if (used >= left) return s + (step * left) / used;
    left -= used;
  }
  return null;
}

/** The run's state in one line, worst first, with what to check when it
 *  needs someone. tone: 'ok' | 'info' | 'warn' | 'bad' | 'idle'. */
export function runState(status, connected, inHold) {
  const s = status ?? {};
  const c = s.trim_c != null ? `×${s.trim_c.toFixed(3)}` : 'its last value';
  const trimmed = !!s.active?.gravimetric_trim;
  if (!connected)
    return {
      tone: 'idle',
      title: 'No news from the daemon',
      detail: 'What this page shows is the last state it reported.',
      check: 'If it does not come back, start Fermentool again: the run resumes where its curve is.',
    };
  if (s.stop_pending)
    return {
      tone: 'bad',
      title: 'Stop not confirmed',
      detail: 'The Stop did not reach the pump: it may still be running.',
      check: 'Fermentool sends it again as soon as the pump answers. Check the pump, stop it by hand if needed.',
    };
  if (s.serial_ok === false)
    return {
      tone: 'bad',
      title: 'Pump not connected',
      detail: 'The pump keeps its last setpoint; the curve is not followed until it answers.',
      check: 'Check power, cable, MODBUS address and baud rate, then Connect.',
    };
  if (trimmed && s.tracking?.wrong_side)
    return {
      tone: 'bad',
      title: 'Balance on the other side',
      detail: 'The weight moves the wrong way for where Settings says the balance is.',
      check: 'This run keeps its pump setpoint, uncorrected. Fix "Balance weighs" in Settings for the next run.',
    };
  if (trimmed && s.tracking?.alarm === 'feed_stopped')
    return {
      tone: 'bad',
      title: 'Feed stopped',
      detail: 'The pump turns, but the bottle weight has not moved for 3 minutes.',
      check: 'Bottle empty? Line pinched, unplugged, or full of air? Tube seated in the head?',
      note: `Correction held at ${c}. Feed missed meanwhile is not caught up.`,
    };
  if (trimmed && (s.tracking?.alarm === 'saturated' || (s.scale_connected !== false && s.scale_ok === false)))
    return {
      tone: 'bad',
      title: 'Correction out of bounds',
      detail: 'For 5 minutes the pump has delivered further from its setpoint than the correction limit can make up.',
      check: 'Check the tube in the head, leaks, the bottle. A wider limit (Settings, Balance) also lifts it.',
      note: `Correction held at ${c}; it resumes by itself once the pump is back within bounds.`,
    };
  if (trimmed && s.scale_connected === false)
    return {
      tone: 'warn',
      title: 'Balance not answering',
      detail: 'The pump keeps following the curve, without correction.',
      check: 'Check the balance cable; Fermentool keeps retrying.',
    };
  if (s.pump_confirmed === false)
    return {
      tone: 'warn',
      title: 'Pump not tracking the setpoint',
      detail: 'Writes get through but the pump reports a different value.',
      check: 'Check the pump.',
    };
  if (s.journal_ok === false)
    return {
      tone: 'warn',
      title: 'Journal stalled',
      detail: 'The pump runs correctly, but the run history is not being saved.',
      check: 'Check free disk space.',
    };
  if (trimmed && (s.scale_state === 'refill_pending' || s.scale_state === 'refill_settling'))
    return {
      tone: 'info',
      title: 'Refilling the bottle',
      detail: 'Correction held while the bottle is refilled; it resumes once the weight is still.',
    };
  if (inHold)
    return {
      tone: 'ok',
      title: 'Curve done, holding its end value',
      detail: 'Still regulated and recorded. Stopping now records the run as completed.',
    };
  return { tone: 'ok', title: 'Running normally' };
}
