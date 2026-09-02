<script>
  import { num } from '../lib/fmt.js';

  /**
   * @type {{
   *   planned?: [number, number][],
   *   actual?: [number, number][],
   *   nowS?: number | null,
   *   durationS: number,
   *   unit?: string,
   *   digits?: number,
   * }}
   */
  let { planned = [], actual = [], nowS = null, durationS, unit = 'rpm', digits = 1 } = $props();

  const W = 960;
  const H = 240;
  const padL = 8;
  const padR = 16;
  const padT = 16;
  const padB = 22;

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

  const line = $derived(planned.map((p, i) => `${i ? 'L' : 'M'}${x(p[0]).toFixed(1)},${y(p[1]).toFixed(1)}`).join(' '));
  const area = $derived(
    planned.length
      ? `${line} L${x(planned[planned.length - 1][0]).toFixed(1)},${H - padB} L${x(planned[0][0]).toFixed(1)},${H - padB} Z`
      : ''
  );
  const nowX = $derived(nowS == null ? null : x(nowS));
  const nowV = $derived.by(() => {
    if (nowS == null || planned.length === 0) return null;
    // nearest planned sample
    let best = planned[0];
    for (const p of planned) if (Math.abs(p[0] - nowS) < Math.abs(best[0] - nowS)) best = p;
    return best[1];
  });
</script>

<div class="chart-wrap">
  <svg viewBox="0 0 {W} {H}" preserveAspectRatio="xMidYMid meet" role="img"
       aria-label="Planned {unit} curve over {Math.round(durationS / 3600)} hours">
    <defs>
      <linearGradient id="ftfill" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0%" stop-color="var(--lime-300)" stop-opacity="0.42" />
        <stop offset="100%" stop-color="var(--lime-300)" stop-opacity="0" />
      </linearGradient>
    </defs>

    {#each [0.25, 0.5, 0.75] as f}
      <line x1={padL} x2={W - padR} y1={padT + f * (H - padT - padB)} y2={padT + f * (H - padT - padB)}
            stroke="var(--line-soft)" stroke-width="1" />
    {/each}

    {#if area}
      <path d={area} fill="url(#ftfill)" />
      <path d={line} fill="none" stroke="var(--teal-400)" stroke-width="2.25" />
    {/if}

    {#if planned.length}
      <circle cx={x(planned[0][0])} cy={y(planned[0][1])} r="3.5" fill="var(--teal-700)" />
      <text x={x(planned[0][0]) + 6} y={H - 6} class="axl">{num(planned[0][1], digits)} {unit}</text>
      <circle cx={x(planned[planned.length - 1][0])} cy={y(planned[planned.length - 1][1])} r="3.5" fill="var(--teal-700)" />
      <text x={W - padR} y="12" text-anchor="end" class="axl">{num(planned[planned.length - 1][1], digits)} {unit}</text>
    {/if}

    {#each actual as p}
      <circle cx={x(p[0])} cy={y(p[1])} r="2.4" fill="var(--green-500)" />
    {/each}

    {#if nowX != null}
      <line x1={nowX} x2={nowX} y1={padT} y2={H - padB}
            stroke="var(--ink)" stroke-opacity="0.25" stroke-width="1" />
      {#if nowV != null}
        <circle cx={nowX} cy={y(nowV)} r="4" fill="var(--surface)"
                stroke="var(--teal-700)" stroke-width="2" />
      {/if}
      {#if nowV != null && nowS > dspan * 0.08}
        <text x={Math.min(nowX, W - padR)} y={H - 7}
              text-anchor={nowX > W - 96 ? 'end' : 'middle'}
              class="axl now">now · {num(nowV, digits)} {unit}</text>
      {/if}
    {/if}
  </svg>
</div>

<style>
  .chart-wrap { overflow-x: auto; }
  svg { width: 100%; height: 220px; display: block; min-width: 520px; }
  .axl { font-family: var(--mono); font-size: 10.5px; fill: var(--muted); }
  .axl.now { fill: var(--teal-700); }
</style>
