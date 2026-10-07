<script>
  import { num, vol } from '../lib/fmt.js';

  /**
   * @type {{
   *   planned?: [number, number][],
   *   actual?: [number, number][],
   *   nowS?: number | null,
   *   durationS: number,
   *   unit?: string,
   *   digits?: number,
   *   nowVolumeMl?: number | null,
   *   actualColor?: string,
   *   actualOpacity?: number,
   *   markers?: { t: number, label: string }[],
   * }}
   */
  let {
    planned = [],
    actual = [],
    nowS = null,
    durationS,
    unit = 'rpm',
    digits = 1,
    nowVolumeMl = null,
    // Stroke of the `actual` trace; the tracking chart sets a contrasting one,
    // slightly see-through so the planned curve shows where they overlap.
    actualColor = 'var(--green-500)',
    actualOpacity = 1,
    // Vertical time markers (e.g. when the regulation acted), dashed, labelled.
    markers = [],
  } = $props();

  // Two charts can share a page (a run's curve and its weighed feed): each
  // needs its own gradient id.
  const fillId = `ftfill-${Math.random().toString(36).slice(2, 9)}`;

  const W = 960;
  const H = 240;
  // No horizontal inset: the plot spans the full width so it lines up edge to
  // edge with the progress bar above it.
  const padL = 0;
  const padR = 0;
  const padT = 16;
  const padB = 22;
  const dotR = 3.5;
  const clampX = (px) => Math.max(dotR, Math.min(W - dotR, px));

  const bounds = $derived.by(() => {
    const vals = [...planned.map((p) => p[1]), ...actual.map((p) => p[1])];
    let lo = Math.min(...vals, Infinity);
    let hi = Math.max(...vals, -Infinity);
    if (!Number.isFinite(lo) || !Number.isFinite(hi)) {
      lo = 0;
      hi = 1;
    }
    if (hi - lo < 1e-6) {
      hi = lo + 1;
    }
    const pad = (hi - lo) * 0.08;
    return { lo: lo - pad, hi: hi + pad };
  });

  const dspan = $derived(durationS > 0 ? durationS : 1);
  const x = (t) => padL + (Math.min(t, dspan) / dspan) * (W - padL - padR);
  const y = (v) => H - padB - ((v - bounds.lo) / (bounds.hi - bounds.lo)) * (H - padT - padB);

  // Endpoint value labels tuck into the corner the curve leaves free: the
  // start label goes opposite the curve's first step, the end label opposite
  // its last step. Correct for rising and falling curves alike.
  const clampY = (py) => Math.max(padT + 9, Math.min(H - 4, py));
  const labels = $derived.by(() => {
    if (planned.length === 0) return null;
    const s = planned[0];
    const e = planned[planned.length - 1];
    const risesFromStart = planned.length > 1 ? planned[1][1] >= s[1] : true;
    const risesToEnd = planned.length > 1 ? e[1] >= planned[planned.length - 2][1] : true;
    return {
      sx: x(s[0]),
      sy: clampY(risesFromStart ? y(s[1]) + 15 : y(s[1]) - 9),
      sVal: s[1],
      ex: x(e[0]),
      ey: clampY(risesToEnd ? y(e[1]) - 9 : y(e[1]) + 15),
      eVal: e[1],
    };
  });

  const line = $derived(planned.map((p, i) => `${i ? 'L' : 'M'}${x(p[0]).toFixed(1)},${y(p[1]).toFixed(1)}`).join(' '));
  const area = $derived(
    planned.length
      ? `${line} L${x(planned[planned.length - 1][0]).toFixed(1)},${H - padB} L${x(planned[0][0]).toFixed(1)},${H - padB} Z`
      : ''
  );
  const nowX = $derived(nowS == null ? null : x(nowS));

  // Value on the planned curve at any time `t`, linearly interpolated between
  // the two bracketing samples so the marker glides continuously instead of
  // snapping from one of the 200 preview points to the next.
  function curveValueAt(t) {
    if (planned.length === 0) return null;
    if (t <= planned[0][0]) return planned[0][1];
    const last = planned[planned.length - 1];
    if (t >= last[0]) return last[1];
    for (let i = 1; i < planned.length; i++) {
      const [t1, v1] = planned[i];
      if (t1 >= t) {
        const [t0, v0] = planned[i - 1];
        const f = t1 === t0 ? 0 : (t - t0) / (t1 - t0);
        return v0 + (v1 - v0) * f;
      }
    }
    return last[1];
  }
  const nowV = $derived(nowS == null ? null : curveValueAt(nowS));

  // The live volume label rides the "now" marker.
  // Horizontally: flips to the left of the dot once it gets close to the
  // right edge, so it never runs off the chart.
  const nowLabelFlip = $derived(nowX != null && nowX > W - 90);
  const nowLabelX = $derived(nowX == null ? null : nowX + (nowLabelFlip ? -10 : 10));
  // Vertically: sits above the dot while it's in the lower half of the plot
  // (plenty of headroom up there), flips below once the dot itself climbs
  // into the upper half, otherwise there's too little room left above it
  // before the top edge. A generous offset matters more than which side:
  // a curve that spends most of a long run settled near-flat close to one
  // edge (e.g. a decelerostat holding near its floor for tens of hours)
  // barely clears its own line at a small offset regardless of side.
  const plotMidY = padT + (H - padT - padB) / 2;
  const nowLabelBelow = $derived(nowV == null ? false : y(nowV) < plotMidY);
  const nowLabelOffset = 22;

  // Numbered marker badges, in time order. Two markers closer than a badge
  // keep their own line but the later badge steps right, so both stay legible.
  const BADGE_R = 7;
  const markerBadges = $derived.by(() => {
    const out = [];
    let lastBx = -Infinity;
    [...markers]
      .sort((a, b) => a.t - b.t)
      .forEach((m, i) => {
        const lx = clampX(x(m.t));
        const bx = Math.min(W - BADGE_R - 1, Math.max(lx, lastBx + 2 * BADGE_R + 3, BADGE_R + 1));
        lastBx = bx;
        out.push({ lx, bx, n: i + 1 });
      });
    return out;
  });

  // Journalled setpoints drawn as one continuous trace rather than a dot per tick.
  const actualLine = $derived(
    actual.length
      ? actual.map((p, i) => `${i ? 'L' : 'M'}${x(p[0]).toFixed(1)},${y(p[1]).toFixed(1)}`).join(' ')
      : ''
  );

  // Live "travelled" trace: the planned curve clipped to `nowS`, ending exactly
  // on the marker. Driven by the same smooth clock as the marker, so the green
  // follows it continuously instead of catching up one tick per second.
  const progressLine = $derived.by(() => {
    if (nowS == null || planned.length === 0) return '';
    const pts = planned.filter((p) => p[0] <= nowS);
    if (nowV != null) pts.push([Math.min(nowS, dspan), nowV]);
    if (pts.length < 2) return '';
    return pts.map((p, i) => `${i ? 'L' : 'M'}${x(p[0]).toFixed(1)},${y(p[1]).toFixed(1)}`).join(' ');
  });
</script>

<div class="chart-wrap">
  <svg viewBox="0 0 {W} {H}" preserveAspectRatio="xMidYMid meet" role="img"
       aria-label="Planned {unit} curve over {Math.round(durationS / 3600)} hours">
    <defs>
      <linearGradient id={fillId} x1="0" y1="0" x2="0" y2="1">
        <stop offset="0%" stop-color="var(--lime-300)" stop-opacity="0.42" />
        <stop offset="100%" stop-color="var(--lime-300)" stop-opacity="0" />
      </linearGradient>
    </defs>

    {#each [0.25, 0.5, 0.75] as f}
      <line x1={padL} x2={W - padR} y1={padT + f * (H - padT - padB)} y2={padT + f * (H - padT - padB)}
            stroke="var(--line-soft)" stroke-width="1" />
    {/each}

    {#if area}
      <path d={area} fill="url(#{fillId})" />
      <path d={line} fill="none" stroke="var(--teal-400)" stroke-width="2.25" />
    {/if}

    {#if labels}
      <circle cx={clampX(labels.sx)} cy={y(planned[0][1])} r={dotR} fill="var(--teal-700)" />
      <text x={clampX(labels.sx) + 6} y={labels.sy} class="axl">{num(labels.sVal, digits)} {unit}</text>
      <circle cx={clampX(labels.ex)} cy={y(planned[planned.length - 1][1])} r={dotR} fill="var(--teal-700)" />
      <text x={clampX(labels.ex) - 6} y={labels.ey} text-anchor="end" class="axl">{num(labels.eVal, digits)} {unit}</text>
    {/if}

    {#if nowS != null && progressLine}
      <path class="progress" d={progressLine} fill="none" stroke="var(--green-500)"
            stroke-width="2.5" stroke-linejoin="round" stroke-linecap="round" />
    {:else if actualLine}
      <path d={actualLine} fill="none" stroke={actualColor} stroke-opacity={actualOpacity} stroke-width="2.5"
            stroke-linejoin="round" stroke-linecap="round" />
    {/if}

    <!-- Markers: a dashed line and a numbered badge in the top margin, out of
         the curves' way; the caller explains the numbers under the chart. -->
    {#each markerBadges as m}
      <line x1={m.lx} x2={m.lx} y1={padT} y2={H - padB} class="marker" />
      <circle cx={m.bx} cy={BADGE_R + 1} r={BADGE_R} class="badge" />
      <text x={m.bx} y={BADGE_R + 4.5} text-anchor="middle" class="badge-n">{m.n}</text>
    {/each}

    {#if nowX != null && nowS < dspan}
      <line x1={nowX} x2={nowX} y1={padT} y2={H - padB}
            stroke="var(--ink)" stroke-opacity="0.25" stroke-width="1" />
      {#if nowV != null}
        <circle cx={nowX} cy={y(nowV)} r="4" fill="var(--surface)"
                stroke="var(--teal-700)" stroke-width="2" />
        {#if nowVolumeMl != null}
          <text x={nowLabelX} y={clampY(y(nowV) + (nowLabelBelow ? nowLabelOffset : -nowLabelOffset))}
                text-anchor={nowLabelFlip ? 'end' : 'start'}
                class="axl now-vol">{vol(nowVolumeMl)}</text>
        {/if}
      {/if}
    {/if}
  </svg>
</div>

<style>
  .chart-wrap { overflow-x: hidden; }
  /* Box matches the viewBox aspect so it fills the full width with no
     letterboxing, lines up edge to edge with the progress bar. */
  svg { width: 100%; aspect-ratio: 960 / 240; height: auto; display: block; }
  .axl { font-family: var(--mono); font-size: 10.5px; fill: var(--muted); }
  .now-vol { fill: var(--ink); font-weight: 600; }
  .marker { stroke: var(--ink); stroke-opacity: 0.45; stroke-width: 1; stroke-dasharray: 4 4; }
  .badge { fill: var(--surface); stroke: var(--ink); stroke-opacity: 0.55; stroke-width: 1; }
  .badge-n { font-family: var(--mono); font-size: 10px; font-weight: 600; fill: var(--ink); }
  .progress { filter: drop-shadow(0 0 3px color-mix(in srgb, var(--green-500) 55%, transparent)); }
</style>
