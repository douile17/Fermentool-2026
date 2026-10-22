// The value axis of a chart (Chart.svelte): from zero, never zoomed onto the
// data. Fitted to its own min and max, a constant 8.33 ml/min run weighed at
// 8.24 to 8.43 filled the whole height, ±1 % read as a wild swing (run 186),
// with no scale to tell. From zero, a 1 % wobble stays a wobble and a pump
// 20 % short still shows.

/** A round step near `raw`: 1, 2, 2.5 or 5 times a power of ten. */
export function niceStep(raw) {
  if (!(raw > 0)) return 1;
  const p = 10 ** Math.floor(Math.log10(raw));
  const f = raw / p;
  return (f <= 1 ? 1 : f <= 2 ? 2 : f <= 2.5 ? 2.5 : f <= 5 ? 5 : 10) * p;
}

/** `{ lo, hi, ticks }` for the values `vals`: `lo` is 0 (or the lowest value,
 *  rounded down, if any is negative), `hi` a round number a little above the
 *  highest, `ticks` the round values in between, `lo` and `hi` included,
 *  about `count` steps. */
export function valueScale(vals, count = 4) {
  const finite = vals.filter((v) => Number.isFinite(v));
  const min = Math.min(0, ...finite);
  let max = Math.max(0, ...finite);
  if (max - min < 1e-9) max = min + 1;
  const step = niceStep(((max - min) * 1.05) / count);
  const lo = Math.floor(min / step + 1e-9) * step;
  const hi = Math.ceil((max * 1.05) / step - 1e-9) * step;
  const ticks = [];
  for (let k = 0; lo + k * step <= hi + step * 1e-6; k++) ticks.push(+(lo + k * step).toPrecision(12));
  return { lo, hi, ticks };
}

/** Elapsed-time ticks for a run's time axis (Overview, New run), `[{ t,
 *  label }]` from 0 to `spanS`: in minutes up to 2 h ("0", "15 min",
 *  "30 min"), in hours past it ("0", "4 h", "8 h"), the step the smallest
 *  round one giving at most `max` intervals, so a 20 min curve and a 100 h
 *  one both read at a glance. Every chart counts the experiment's time,
 *  never the time of day (History's clock times were dropped, 2026-10-22). */
export function elapsedTicks(spanS, max = 6) {
  max = Math.max(1, max);
  if (!(spanS > 0)) return [];
  const inMin = spanS <= 7200;
  const steps = inMin ? [1, 2, 5, 10, 15, 20, 30].map((m) => m * 60) : [1, 2, 3, 4, 6, 8, 12, 24, 48].map((h) => h * 3600);
  const step = steps.find((s) => spanS / s <= max) ?? steps.at(-1);
  const out = [];
  for (let t = 0; t <= spanS + 1e-6; t += step) {
    out.push({ t, label: t === 0 ? '0' : inMin ? `${t / 60} min` : `${t / 3600} h` });
  }
  return out;
}

/** How many time intervals a chart `widthPx` wide can label: one per 80 px,
 *  6 at most. "100 min" and "120 min" overlapped on New run's narrow
 *  preview (2026-10-22). */
export const ticksFor = (widthPx) => Math.min(6, Math.max(2, Math.floor(widthPx / 80)));
