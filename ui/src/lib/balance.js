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
