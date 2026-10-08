<script>
  // The balance regulation of a live trimmed run, three charts on one time
  // axis so a moment reads straight down: the correction factor c, the gap
  // between weighed and requested volume, the balance weight. One "now"
  // line and the regulation's events cross all three.
  //
  // c is not journalled as such: each tick holds the setpoint written, i.e.
  // curve(t) x c snapped to the pump's grid, so c is read back as target /
  // curve. The snapping adds a little noise at low flows (0.001 ml/min on
  // 0.3 ml/min is 0.3 %); a median of 5 samples takes it out and keeps the
  // steps.
  import { get } from '../lib/api.js';
  import { num, dur } from '../lib/fmt.js';
  import { valueAt, when, toleranceMl } from '../lib/runview.js';

  let {
    runId,
    planned = [],
    durationS,
    nowS,
    startedAtMs,
    limitPct = null,
    feedSide = true,
    emptyInS = null,
    // The run's tracking report, loaded by the page (it states the totals too).
    report = null,
    // The latest values from the status frame, every second: appended to the
    // journalled series so the three charts move as the run does.
    // { c, weightG, askedMl, weighedMl }
    live = null,
  } = $props();

  let ticks = $state([]);

  // Every second, as the rest of the page, never two requests at once. On the
  // API's own SQLite connection, never the control thread.
  $effect(() => {
    const id = runId;
    let stop = false;
    let busy = false;
    const load = () => {
      if (busy) return;
      busy = true;
      get(`/api/runs/${id}/ticks?sample=600`)
        .then((t) => !stop && (ticks = t ?? []))
        .catch(() => {})
        .finally(() => (busy = false));
    };
    load();
    const t = setInterval(load, 1_000);
    return () => {
      stop = true;
      clearInterval(t);
    };
  });

  const W = 1000;
  const L = 64;
  const R = 70;
  const PH = 104;
  const HEAD = 26;
  const GAP = 30;
  const BADGES = 22;
  // Top of each plot; its title row sits HEAD above it.
  const tops = [0, 1, 2].map((i) => BADGES + HEAD + i * (HEAD + PH + GAP));
  const H = tops[2] + PH + 28;

  const span = $derived(Math.max(durationS, nowS, report?.points.at(-1)?.[0] ?? 0, 1));
  const x = (t) => L + (Math.min(Math.max(t, 0), span) / span) * (W - L - R);
  const xNow = $derived(x(nowS));

  // Hour ticks at a round step, about five across.
  const hourTicks = $derived.by(() => {
    const h = span / 3600;
    const step = [1, 2, 5, 10, 12, 24, 48].find((s) => h / s <= 6) ?? 96;
    const out = [];
    for (let v = 0; v <= h + 1e-9; v += step) out.push(v);
    return out;
  });

  // A journalled series with the live point added at its end, when newer.
  const withLive = (pts, p) => (p && (!pts.length || p[0] > pts.at(-1)[0]) ? [...pts, p] : pts);

  const line = (pts) => pts.map(([px, py], i) => `${i ? 'L' : 'M'}${px.toFixed(1)},${py.toFixed(1)}`).join(' ');

  // --- c, from the ticks
  const cPts = $derived.by(() => {
    const raw = [];
    for (const k of ticks) {
      const v = valueAt(planned, k.elapsed_s);
      if (v > 1e-9 && k.target != null) raw.push([k.elapsed_s, k.target / v]);
    }
    const out = raw.map(([t], i) => {
      const w = raw.slice(Math.max(0, i - 2), i + 3).map((p) => p[1]).sort((a, b) => a - b);
      return [t, w[w.length >> 1]];
    });
    return withLive(out, live?.c != null ? [nowS, live.c] : null);
  });
  const cBounds = $derived(limitPct != null ? [1 / (1 + limitPct / 100), 1 + limitPct / 100] : null);
  const cRange = $derived.by(() => {
    let lo = 0.95;
    let hi = 1.05;
    for (const [, c] of cPts) {
      lo = Math.min(lo, c);
      hi = Math.max(hi, c);
    }
    const pad = (hi - lo) * 0.08;
    return [lo - pad, hi + pad];
  });
  const cy = (c) => tops[0] + PH * (1 - (c - cRange[0]) / (cRange[1] - cRange[0]));
  const cNow = $derived(cPts.at(-1)?.[1] ?? null);
  const inC = (c) => c >= cRange[0] && c <= cRange[1];
  // The c axis labels, most telling first; one closer than 13 px to a label
  // already kept is dropped, so ×1.50 and ×1.58 never print on each other.
  const cLabels = $derived.by(() => {
    const want = [1, ...(cBounds ?? []).filter(inC), cRange[1], cRange[0]];
    const kept = [];
    for (const v of want) if (kept.every((k) => Math.abs(cy(k) - cy(v)) >= 13)) kept.push(v);
    return kept;
  });

  // --- gap, from the tracking report
  const gapPts = $derived(
    withLive(
      report ? report.points.map((p) => [p[0], p[2] - p[1], p[1]]) : [],
      live?.askedMl != null ? [nowS, live.weighedMl - live.askedMl, live.askedMl] : null,
    ),
  );
  const gMax = $derived.by(() => {
    let m = 0.2;
    for (const [, d, r] of gapPts) m = Math.max(m, Math.abs(d), toleranceMl(r));
    return m * 1.1;
  });
  const gy = (g) => tops[1] + PH / 2 - (g / gMax) * (PH / 2);
  const gapBand = $derived.by(() => {
    if (gapPts.length < 2) return '';
    const top = gapPts.map(([t, , r]) => [x(t), gy(toleranceMl(r))]);
    const bot = [...gapPts].reverse().map(([t, , r]) => [x(t), gy(-toleranceMl(r))]);
    return `${line(top)} ${bot.map(([a, b]) => `L${a.toFixed(1)},${b.toFixed(1)}`).join(' ')} Z`;
  });
  const gNow = $derived(gapPts.at(-1)?.[1] ?? null);

  // --- balance weight, from the ticks
  const wPts = $derived(
    withLive(
      ticks.filter((k) => k.weight_g != null).map((k) => [k.elapsed_s, k.weight_g]),
      live?.weightG != null ? [nowS, live.weightG] : null,
    ),
  );
  const wMax = $derived(Math.max(100, ...wPts.map((p) => p[1])) * 1.08);
  const wy = (w) => tops[2] + PH * (1 - Math.max(0, w) / wMax);
  const wNow = $derived(wPts.at(-1)?.[1] ?? null);
  // Straight dashed line to the moment it runs dry, when that is on the chart.
  const emptyAt = $derived(feedSide && emptyInS != null ? nowS + emptyInS : null);
  const emptyOnChart = $derived(emptyAt != null && emptyAt <= span);

  // What the regulation did, in time order, numbered like the chart badges.
  const LABELS = {
    trim_start: 'correction starts',
    trim_ratio: 'pump ratio measured',
    alarm_feed_stopped: 'feed stopped',
    alarm_saturated: 'correction out of bounds',
    alarm_wrong_side: 'balance on the other side',
    alarm_cleared: 'regulation resumed',
    refill: 'bottle refilled',
  };
  const marks = $derived(
    [
      ...(report?.markers ?? []).filter((m) => m.kind !== 'refill').map((m) => ({ t: m.t_s, label: LABELS[m.kind] ?? m.kind })),
      ...(report?.refills ?? []).map((r) => ({ t: r.t_s, label: `bottle refilled, ${num(r.before_g, 0)} → ${num(r.after_g, 0)} g` })),
    ].sort((a, b) => a.t - b.t),
  );
  // Badges of events close in time sit side by side instead of on top of
  // each other; each keeps a short leader down to its event.
  const badgeX = $derived.by(() => {
    const out = [];
    for (const m of marks) out.push(Math.max(x(m.t), (out.at(-1) ?? -Infinity) + 17));
    return out;
  });
</script>

{#if report || ticks.length}
  <div class="reg">
    <svg viewBox="0 0 {W} {H}" role="img"
         aria-label="Regulation over the run on one time axis: correction factor, gap between weighed and requested volume, balance weight">
      <!-- panel titles, each on its own row above its plot, and the hour axis -->
      {#each [['Correction c', 'factor on the curve'], ['Gap', 'weighed minus asked, mL'], ['Balance', 'g']] as [t, u], i}
        {#if i}<line class="sep" x1="0" x2={W} y1={tops[i] - HEAD - GAP / 2} y2={tops[i] - HEAD - GAP / 2} />{/if}
        <text class="title" x="0" y={tops[i] - 9}>{t}<tspan class="unit" dx="8">{u}</tspan></text>
      {/each}
      {#each hourTicks as h}
        <text class="axl" x={x(h * 3600)} y={H - 6} text-anchor="middle">{h} h</text>
      {/each}

      <!-- c: the allowed range where it falls on the scale, the value -->
      {#if cBounds}
        {@const zTop = cy(Math.min(cBounds[1], cRange[1]))}
        {@const zBot = cy(Math.max(cBounds[0], cRange[0]))}
        <rect class="zone" x={L} y={zTop} width={W - L - R} height={Math.max(0, zBot - zTop)} />
        {#each cBounds as b}
          {#if inC(b)}<line class="limit" x1={L} x2={W - R} y1={cy(b)} y2={cy(b)} />{/if}
        {/each}
        {#if !inC(cBounds[0]) || !inC(cBounds[1])}
          <text class="axl" x={W - R} y={tops[0] - 9} text-anchor="end">limit ±{limitPct} %</text>
        {/if}
      {/if}
      <line class="grid" x1={L} x2={W - R} y1={cy(1)} y2={cy(1)} />
      {#each cLabels as v}
        <text class="axl" x={L - 6} y={Math.min(tops[0] + PH, Math.max(tops[0] + 9, cy(v) + 4))} text-anchor="end">×{v.toFixed(2)}</text>
      {/each}
      {#if cPts.length > 1}
        <path class="c" d={line(cPts.map(([t, c]) => [x(t), cy(c)]))} />
      {/if}

      <!-- gap: tolerance funnel around zero -->
      {#if gapBand}<path class="zone" d={gapBand} />{/if}
      <line class="grid" x1={L} x2={W - R} y1={gy(0)} y2={gy(0)} />
      <text class="axl" x={L - 6} y={gy(0) + 4} text-anchor="end">0</text>
      <text class="axl" x={L - 6} y={tops[1] + 10} text-anchor="end">+{num(gMax, gMax < 10 ? 1 : 0)}</text>
      <text class="axl" x={L - 6} y={tops[1] + PH} text-anchor="end">−{num(gMax, gMax < 10 ? 1 : 0)}</text>
      {#if gapPts.length > 1}
        <path class="gap" d={line(gapPts.map(([t, g]) => [x(t), gy(g)]))} />
      {/if}

      <!-- balance weight, and where it runs dry -->
      <line class="grid" x1={L} x2={W - R} y1={wy(0)} y2={wy(0)} />
      <text class="axl" x={L - 6} y={wy(0) + 4} text-anchor="end">0</text>
      <text class="axl" x={L - 6} y={tops[2] + 10} text-anchor="end">{num(wMax, 0)}</text>
      {#if wPts.length > 1}
        <path class="w" d={line(wPts.map(([t, w]) => [x(t), wy(w)]))} />
      {/if}
      {#if wNow != null && emptyOnChart}
        <path class="w proj" d={line([[xNow, wy(wNow)], [x(emptyAt), wy(0)]])} />
        <circle class="empty" cx={x(emptyAt)} cy={wy(0)} r="4" />
      {/if}

      <!-- events, through all three panels -->
      {#each marks as m, i}
        <!-- through the plots only, never across a title row -->
        <line class="mark" x1={badgeX[i]} x2={x(m.t)} y1={BADGES / 2 + 7} y2={BADGES} />
        {#each tops as top}
          <line class="mark" x1={x(m.t)} x2={x(m.t)} y1={top} y2={top + PH} />
        {/each}
        <circle class="badge" cx={badgeX[i]} cy={BADGES / 2} r="7" />
        <text class="badge-n" x={badgeX[i]} y={BADGES / 2 + 3.5} text-anchor="middle">{i + 1}</text>
      {/each}

      <!-- now -->
      {#each tops as top}
        <line class="now" x1={xNow} x2={xNow} y1={top} y2={top + PH} />
      {/each}
      {#if cNow != null}
        <circle class="dot c" cx={xNow} cy={cy(cNow)} r="4" />
        <text class="val" x={xNow + 8} y={Math.max(tops[0] + 10, cy(cNow) - 6)}>×{cNow.toFixed(3)}</text>
      {/if}
      {#if gNow != null}
        <circle class="dot gap" cx={xNow} cy={gy(gNow)} r="4" />
        <text class="val" x={xNow + 8} y={Math.max(tops[1] + 10, gy(gNow) - 6)}>{gNow >= 0 ? '+' : '−'}{num(Math.abs(gNow), 1)} mL</text>
      {/if}
      {#if wNow != null}
        <circle class="dot w" cx={xNow} cy={wy(wNow)} r="4" />
        <text class="val" x={xNow + 8} y={Math.max(tops[2] + 10, wy(wNow) - 6)}>{num(wNow, 1)} g</text>
      {/if}
    </svg>

    {#if marks.length || emptyAt != null}
      <ol class="key">
        {#each marks as m, i}
          <li><i>{i + 1}</i>{m.label}<span class="mono">{when(startedAtMs + m.t * 1000)}</span></li>
        {/each}
        {#if emptyAt != null}
          <li class="dry"><i></i>runs dry<span class="mono">{when(startedAtMs + emptyAt * 1000)}</span></li>
        {/if}
      </ol>
    {/if}
    {#if report?.points.length && report.points.at(-1)[0] > durationS + 1}
      <p class="held">Includes {dur(report.points.at(-1)[0] - durationS)} of hold at the curve's end value.</p>
    {/if}
  </div>
{/if}

<style>
  .reg { display: flex; flex-direction: column; gap: var(--s-3); min-width: 0; }
  svg { width: 100%; height: auto; display: block; overflow: visible; }
  .title { font-size: 13px; font-weight: 600; fill: var(--ink); }
  .unit { font-size: 11.5px; font-weight: 400; fill: var(--muted); }
  .sep { stroke: var(--line-soft); stroke-width: 1; }
  .limit { stroke: var(--teal-400); stroke-width: 1; stroke-dasharray: 3 3; opacity: 0.7; }
  .axl { font-family: var(--mono); font-size: 10.5px; fill: var(--muted); }
  .grid { stroke: var(--line); stroke-width: 1; }
  .zone { fill: color-mix(in srgb, var(--teal-400) 13%, transparent); stroke: none; }
  .c { fill: none; stroke: var(--teal-700); stroke-width: 2; }
  .gap { fill: none; stroke: var(--measured); stroke-width: 1.6; }
  .w { fill: none; stroke: var(--ink); stroke-width: 1.6; opacity: 0.75; }
  .w.proj { stroke-dasharray: 4 4; opacity: 0.4; }
  .empty { fill: var(--measured); }
  .mark { stroke: var(--muted); stroke-width: 1; stroke-dasharray: 2 3; opacity: 0.6; }
  .badge { fill: var(--surface); stroke: var(--muted); stroke-width: 1; }
  .badge-n { font-family: var(--mono); font-size: 9.5px; font-weight: 600; fill: var(--ink); }
  .now { stroke: var(--ink); stroke-width: 1; }
  .dot { fill: var(--surface); stroke-width: 2.2; }
  .dot.c { stroke: var(--teal-700); }
  .dot.gap { stroke: var(--measured); }
  .dot.w { stroke: var(--ink); }
  .val { font-family: var(--mono); font-size: 11px; font-weight: 500; fill: var(--ink); }

  .key {
    list-style: none; margin: 0; padding: 0;
    display: flex; flex-wrap: wrap; gap: var(--s-2) var(--s-5);
    font-size: 12.5px; color: var(--muted);
  }
  .key li { display: inline-flex; align-items: center; gap: 6px; }
  .key i {
    font-style: normal; font-family: var(--mono); font-size: 10px; font-weight: 600; color: var(--ink);
    width: 16px; height: 16px; border-radius: 50%; border: 1px solid var(--muted);
    display: inline-grid; place-items: center;
  }
  .key .dry i { border: none; background: var(--measured); width: 8px; height: 8px; }
  .key .mono { color: var(--ink); }
  .held { margin: 0; font-size: 12px; color: var(--muted); }
</style>
