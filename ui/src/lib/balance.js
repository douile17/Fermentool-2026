// The bottle's weights over a run, from a tracking report: start, each refill
// (before -> after), end. `weighedOut` sums the falls between them, plus what
// the feed drew while each refill was poured (`drawn_g`, counted by the
// daemon: the balance cannot see it under the rising weight), to check
// against the daemon's `delivered` by hand. Each refill gets `added_g`, what
// was really poured: its net gain plus that draw. Negative when the balance
// weighs the receiving vessel (its weight rises).
export function bottleWeights(report) {
  if (!report || report.weight_start_g == null || report.weight_end_g == null) return null;
  const refills = (report.refills ?? []).map((r) => {
    const drawn = r.drawn_g > 0 ? r.drawn_g : 0;
    return { ...r, drawn_g: drawn, added_g: r.after_g - r.before_g + drawn };
  });
  let from = report.weight_start_g;
  let weighedOut = 0;
  for (const r of refills) {
    weighedOut += from - r.before_g + r.drawn_g;
    from = r.after_g;
  }
  weighedOut += from - report.weight_end_g;
  return { start: report.weight_start_g, end: report.weight_end_g, refills, weighedOut };
}

/** How long after a refill the daemon's figure stays on: the tracking
 *  report, refetched every ~10 s, must first carry the new refill. */
export const REFILL_GRACE_MS = 20_000;

/** What the Feed bottle panel shows as weighed out: `bottleOut`, its own
 *  count from the bottle's weights, except while a refill runs (and
 *  `REFILL_GRACE_MS` after). Then the rising weight is not yet a journalled
 *  refill and that count fell as the bottle filled (run 194: ~200 g under
 *  the carboy mid-refill); the daemon's weighed delivery, which estimates
 *  the feed through the refill, is shown instead, said estimated. */
export function weighedOutShown({ bottleOut, deliveredG, refilling }) {
  if (refilling && deliveredG != null) return { g: deliveredG, estimated: true };
  return { g: bottleOut ?? null, estimated: false };
}
