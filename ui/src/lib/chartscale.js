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
