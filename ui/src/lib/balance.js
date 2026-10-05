// The bottle's weights over a run, from a tracking report: start, each refill
// (before -> after), end. `weighedOut` sums the falls between them, i.e. what
// left the bottle according to the balance alone, to check against the
// daemon's `delivered` by hand. Negative when the balance weighs the
// receiving vessel (its weight rises).
export function bottleWeights(report) {
  if (!report || report.weight_start_g == null || report.weight_end_g == null) return null;
  const refills = report.refills ?? [];
  let from = report.weight_start_g;
  let weighedOut = 0;
  for (const r of refills) {
    weighedOut += from - r.before_g;
    from = r.after_g;
  }
  weighedOut += from - report.weight_end_g;
  return { start: report.weight_start_g, end: report.weight_end_g, refills, weighedOut };
}
