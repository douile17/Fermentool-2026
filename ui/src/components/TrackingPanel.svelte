<script>
  // Delivered (weighed) vs requested feed volume for one run, with R², the
  // cumulative deficit and, for an exponential curve, requested vs fitted µ.
  // Renders nothing for a run without balance data (the API answers 404).
  import { get } from '../lib/api.js';
  import { num, dur } from '../lib/fmt.js';
  import { app } from '../lib/state.svelte.js';
  import Chart from './Chart.svelte';

  let { runId, durationS, live = false } = $props();
  let report = $state(null);

  $effect(() => {
    const id = runId;
    let stop = false;
    const load = () =>
      get(`/api/runs/${id}/tracking`)
        .then((r) => {
          if (!stop) report = r;
        })
        .catch(() => {
          if (!stop) report = null;
        });
    load();
    // While running, refresh on the controller's own 10 s cadence.
    const t = live ? setInterval(load, 10_000) : null;
    return () => {
      stop = true;
      if (t) clearInterval(t);
    };
  });

  const requested = $derived(report ? report.points.map((p) => [p[0], p[1]]) : []);
  const delivered = $derived(report ? report.points.map((p) => [p[0], p[2]]) : []);
  // A run goes on past its curve (hold phase): the axis spans whatever was
  // recorded, not just the curve.
  const spanS = $derived(Math.max(durationS, report?.points.at(-1)?.[0] ?? 0));
  const heldS = $derived(Math.max(0, spanS - durationS));

  // The plain answer first: how much was asked for, how much arrived.
  const last = $derived(report?.points.at(-1) ?? null);
  const reqMl = $derived(last ? last[1] : null);
  const delMl = $derived(last ? last[2] : null);
  const ratioPct = $derived(reqMl > 0 ? (100 * delMl) / reqMl : null);
  const wrongSide = $derived(
    (live && app.status?.active?.run_id === runId && app.status?.tracking?.wrong_side) ||
      (delMl != null && reqMl > 2 && delMl < -0.25 * reqMl),
  );

  // Gap chart: two cumulative curves that agree to 0.5 mL over 60 mL overlap
  // to the pixel, so "on target?" is drawn as the gap itself, around a zero
  // line, inside a ±TOL band that widens with the volume requested so far.
  const TOL = 0.02;
  const GW = 960;
  const GH = 110;
  const gPad = 8;
  const gap = $derived(report ? report.points.map((p) => [p[0], p[2] - p[1], p[1]]) : []);
  const gMax = $derived.by(() => {
    let m = 0.2;
    for (const [, d, r] of gap) m = Math.max(m, Math.abs(d), TOL * r);
    return m * 1.1;
  });
  const gx = (t) => (Math.min(t, spanS) / (spanS || 1)) * GW;
  const gy = (v) => gPad + ((gMax - v) / (2 * gMax)) * (GH - 2 * gPad);
  const gapLine = $derived(
    gap.map(([t, d], i) => `${i ? 'L' : 'M'}${gx(t).toFixed(1)},${gy(d).toFixed(1)}`).join(' '),
  );
  const bandPath = $derived.by(() => {
    if (gap.length < 2) return '';
    const top = gap.map(([t, , r], i) => `${i ? 'L' : 'M'}${gx(t).toFixed(1)},${gy(TOL * r).toFixed(1)}`);
    const bottom = [...gap].reverse().map(([t, , r]) => `L${gx(t).toFixed(1)},${gy(-TOL * r).toFixed(1)}`);
    return `${top.join(' ')} ${bottom.join(' ')} Z`;
  });
  // When the regulation acted, from the daemon's journal.
  const MARKER_LABELS = { trim_start: 'correction starts', trim_ratio: 'pump ratio measured' };
  // In time order: the chart numbers its badges the same way.
  const markers = $derived(
    (report?.markers ?? [])
      .map((m) => ({ t: m.t_s, label: `${MARKER_LABELS[m.kind] ?? m.kind} · ${dur(m.t_s)}` }))
      .sort((a, b) => a.t - b.t),
  );

  const gapNow = $derived(delMl != null && reqMl != null ? delMl - reqMl : null);
  const inBand = $derived(gapNow != null && Math.abs(gapNow) <= TOL * Math.max(reqMl, 0));
</script>

{#if report}
  <div class="track">
    <div class="eyebrow">Feed delivered vs requested (weighed)</div>
    {#if wrongSide}
      <p class="warn">
        The balance weight moved the wrong way for where Settings says it is (feed bottle or
        receiving vessel), so the numbers below are mirrored and meaningless. Set
        <b>Balance weighs</b> in Settings to match the bench for the next run.
      </p>
    {:else if ratioPct != null}
      <!-- The one place the run's volume is stated: weighed vs requested. -->
      <p class="headline">
        <b>{num(delMl, 1)} mL</b> delivered of <b>{num(reqMl, 1)} mL</b> requested ·
        <b class:off={!inBand}>{num(ratioPct, 1)} %</b> · gap
        <b class="mono" class:ok={inBand} class:off={!inBand}>{gapNow >= 0 ? '+' : ''}{num(gapNow, 2)} mL</b>,
        {inBand ? 'within' : 'outside'} ±{TOL * 100} %
      </p>
    {/if}
    <div class="legend">
      <span><i class="sw req"></i>requested</span>
      <span><i class="sw meas"></i>delivered (weighed)</span>
    </div>
    <Chart planned={requested} actual={delivered} nowS={null} durationS={spanS} unit="mL" digits={1}
           actualColor="var(--measured)" actualOpacity={0.8} {markers} />
    {#if markers.length}
      <div class="marker-key">
        {#each markers as m, i}
          <span><i class="num">{i + 1}</i>{m.label}</span>
        {/each}
      </div>
    {/if}
    {#if !wrongSide && gap.length > 1}
      <div class="gap-head"><span>Gap, delivered − requested</span></div>
      <svg class="gap" viewBox="0 0 {GW} {GH}" preserveAspectRatio="none" role="img"
           aria-label="Gap between delivered and requested volume, with a ±{TOL * 100} % band">
        <path d={bandPath} class="band" />
        {#each markers as m}
          <line x1={gx(m.t)} x2={gx(m.t)} y1="0" y2={GH} class="mark" />
        {/each}
        <line x1="0" x2={GW} y1={gy(0)} y2={gy(0)} class="zero" />
        <path d={gapLine} class="gapline" />
      </svg>
      <div class="gap-axis mono">
        <span>+{num(gMax, 2)} mL ahead</span>
        <span>shaded: ±{TOL * 100} % of requested · line: on target</span>
        <span>−{num(gMax, 2)} mL behind</span>
      </div>
    {/if}
    {#if heldS > 1}
      <p class="held">Includes {dur(heldS)} of hold at the curve's end value, after the curve.</p>
    {/if}
    <div class="stats mono">
      <span title="How closely the delivered curve follows the requested one: 1 = exactly">
        R² <b>{report.r_squared != null ? report.r_squared.toFixed(5) : '·'}</b>
      </span>
      {#if report.mu_requested != null}
        <span>µ requested <b>{report.mu_requested.toFixed(4)} h⁻¹</b></span>
        <span>
          µ delivered <b>{report.mu_delivered != null ? report.mu_delivered.toFixed(4) : '·'} h⁻¹</b>
        </span>
      {/if}
    </div>
  </div>
{/if}

<style>
  .track {
    margin-top: var(--s-5);
    padding-top: var(--s-5);
    border-top: 1px solid color-mix(in srgb, var(--muted) 32%, transparent);
  }
  .track .eyebrow { margin-bottom: var(--s-3); color: var(--ink); }
  .headline { font-size: 14px; margin: 0 0 var(--s-3); }
  .held { font-size: 12px; color: var(--muted); margin: var(--s-2) 0 0; }
  .legend { display: flex; gap: var(--s-4); font-size: 12px; color: var(--muted); margin-bottom: var(--s-2); }
  .legend span { display: inline-flex; align-items: center; gap: 6px; }
  .sw { display: inline-block; width: 16px; height: 3px; border-radius: 2px; }
  .sw.req { background: var(--teal-400); }
  .sw.meas { background: var(--measured); }
  .marker-key { display: flex; flex-wrap: wrap; gap: var(--s-1) var(--s-4); font-size: 12px; color: var(--muted); margin-top: var(--s-2); }
  .marker-key span { display: inline-flex; align-items: center; gap: 6px; }
  .num {
    display: inline-grid;
    place-items: center;
    width: 16px;
    height: 16px;
    border-radius: 999px;
    border: 1px solid color-mix(in srgb, var(--ink) 55%, transparent);
    font: 600 10px var(--mono);
    font-style: normal;
    color: var(--ink);
  }
  .headline .off { color: var(--danger); }
  .headline .ok { color: var(--green-600); }
  .gap-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: var(--s-3);
    flex-wrap: wrap;
    font-size: 12px;
    color: var(--muted);
    margin: var(--s-4) 0 var(--s-2);
  }
  .gap { width: 100%; height: 110px; display: block; }
  .gap .band { fill: color-mix(in srgb, var(--teal-400) 18%, transparent); stroke: none; }
  .gap .zero { stroke: var(--teal-400); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
  .gap .mark {
    stroke: var(--ink);
    stroke-opacity: 0.45;
    stroke-width: 1;
    stroke-dasharray: 4 4;
    vector-effect: non-scaling-stroke;
  }
  .gap .gapline {
    fill: none;
    stroke: var(--measured);
    stroke-width: 2;
    stroke-linejoin: round;
    vector-effect: non-scaling-stroke;
  }
  .gap-axis {
    display: flex;
    justify-content: space-between;
    gap: var(--s-2);
    font-size: 10.5px;
    color: var(--muted);
    margin-top: 2px;
  }
  @media (max-width: 720px) {
    .gap-axis span:nth-child(2) { display: none; }
  }
  .warn { font-size: 13px; color: var(--danger); margin: 0 0 var(--s-3); line-height: 1.4; }
  .stats {
    display: flex;
    flex-wrap: wrap;
    gap: var(--s-2) var(--s-5);
    font-size: 13px;
    margin-top: var(--s-3);
  }
</style>
