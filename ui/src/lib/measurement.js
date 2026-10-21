// One measurement of a tubing calibration (a "burst" in the code and the
// API): the pump runs a set time into or out of the bottle on the balance,
// which is read every second. Its profile is a diagnostic only; the
// recorded weight stays the before / after difference.

/** Start and end transients left out of the slope, seconds. */
export const SKIP_START_S = 15;
export const SKIP_END_S = 3;

/** Least-squares line through `[x, y]` points: `{ k, b0 }`, null under 5 points. */
export function fitLine(pts) {
  const n = pts.length;
  if (n < 5) return null;
  const mx = pts.reduce((a, p) => a + p[0], 0) / n;
  const my = pts.reduce((a, p) => a + p[1], 0) / n;
  let sxy = 0;
  let sxx = 0;
  for (const [x, y] of pts) {
    sxy += (x - mx) * (y - my);
    sxx += (x - mx) ** 2;
  }
  if (!(sxx > 0)) return null;
  const k = sxy / sxx;
  return { k, b0: my - k * mx };
}

/** A measurement's balance profile from its journal ticks: the grams
 *  collected (or drawn) since its start every second, `pts`; the flow as
 *  the slope over the steady middle, `gPerMin`, with its line `fit` (in
 *  collected grams); how much it changed from the first half to the second,
 *  `driftPct` (null when a half moved too few balance steps to have a
 *  slope); `endS`. null when the balance did not follow the measurement
 *  (one stale weight throughout, from before it was read mid-burst). */
export function measurementProfile(ticks, resG) {
  const read = (ticks ?? []).filter((t) => t.weight_g != null).sort((a, b) => a.elapsed_s - b.elapsed_s);
  if (!read.length) return null;
  const endS = Math.max(...read.map((t) => t.elapsed_s));
  const w0 = read[0].weight_g;
  // Drawn from the bottle or pumped into it: collected is the change, positive.
  const sign = read.at(-1).weight_g <= w0 ? 1 : -1;
  const pts = read.map((t) => [t.elapsed_s, sign * (w0 - t.weight_g)]);
  const mid = pts.filter(([t]) => t >= SKIP_START_S && t <= endS - SKIP_END_S);
  if (new Set(mid.map((p) => p[1])).size < 3) return null;
  const fit = fitLine(mid);
  if (!fit) return null;
  const h = mid.length >> 1;
  const first = mid.slice(0, h);
  const second = mid.slice(h);
  const a = fitLine(first);
  const z = fitLine(second);
  // A half that moved only a few balance steps has no slope to compare:
  // dividing by its near-zero one gave "+1.98e+28 %" on a dry burst.
  const span = (s) => (s.length > 1 ? s[s.length - 1][0] - s[0][0] : 0);
  const enough = (f, s) => f != null && Math.abs(f.k) * span(s) >= 5 * resG;
  return {
    pts,
    fit,
    endS,
    gPerMin: Math.abs(fit.k) * 60,
    driftPct: enough(a, first) && enough(z, second) ? (100 * (Math.abs(z.k) - Math.abs(a.k))) / Math.abs(a.k) : null,
  };
}

/** Which measurement the detail opens on: the one being dealt with (pumping,
 *  to weigh, or next), else the last one weighed. */
export const defaultMeasurement = (cards) => {
  const cur = cards.findIndex((c) => !c.weighed);
  return cur >= 0 ? cur : cards.length - 1;
};

// --- weighing: the mean of the stable readings, not one reading ------------

/** How far back the weighing averages, ms. */
export const AVG_WINDOW_MS = 5000;

/** The stable readings kept for the weighing, `[{ t, w }]` (ms, g): a
 *  reading the balance calls stable is added, those older than `windowMs`
 *  drop out, and anything else (moving, no reading) clears it all. One
 *  reading taken when the balance said stable could be 27.4 or 27.6 g on a
 *  balance flickering ±0.1 g: the mean of the last few is repeatable. */
export function pushStable(buf, { t, w, stable }, windowMs = AVG_WINDOW_MS) {
  if (w == null || !Number.isFinite(w) || !stable) return [];
  return [...(buf ?? []), { t, w }].filter((r) => r.t > t - windowMs);
}

/** The mean of the kept readings, `{ g, n }`, or null when there are none. */
export function stableMean(buf) {
  if (!buf?.length) return null;
  return { g: buf.reduce((a, r) => a + r.w, 0) / buf.length, n: buf.length };
}

// --- redo all three -----------------------------------------------------------

/** What "Redo all 3" drops, for its confirmation: the weights and the runs
 *  of the measurements pumped so far, in slot order. `{ weights, runs, n }`. */
export function redoAllSummary(bursts) {
  const done = [...(bursts ?? [])].sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0));
  return {
    n: done.length,
    weights: done.filter((b) => b.weight_g != null).map((b) => b.weight_g),
    runs: done.map((b) => b.run_id),
  };
}

/** The session after "Redo all 3": every measurement dropped, the tube and
 *  the pump setup kept (unlike Abandon, which clears them too). */
export const withoutMeasurements = (draft) => ({ ...draft, bursts: [] });
