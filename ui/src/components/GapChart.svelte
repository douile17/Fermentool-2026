<script>
  // A past run's running total against the request: delivered − requested
  // (mL) since the start, averaged over 5 min, on a scale symmetric around 0
  // with labelled gridlines. Refills are shaded and their stretch dashed
  // grey and straight: the balance cannot weigh the feed there.
  import { num } from '../lib/fmt.js';
  import { splitEstimated } from '../lib/refill.js';
  import { gapSeries, gapScale } from '../lib/rundetail.js';
  import { elapsedTicks, ticksFor, axisGutter } from '../lib/chartscale.js';

  /** @type {{ points: [number, number, number][], spans?: [number, number][], spanS: number, ticks?: { t: number, label: string }[], minGutter?: number, gutter?: number }} */
  // `minGutter` / `gutter`: as in Chart.svelte, the two charts' margins agree.
  let { points, spans = [], spanS, ticks = null, minGutter = 0, gutter = $bindable(0) } = $props();

  let boxW = $state(700);
  const W = $derived(Math.max(240, Math.round(boxW)));
  const H = 120;
  const padT = 8;
  const padB = 22;

  const series = $derived(gapSeries(points, spans));
  const scale = $derived(gapScale(series));
  const span = $derived(spanS > 0 ? spanS : 1);
  const digits = $derived(scale.ticks.length > 1 && scale.ticks[1] - scale.ticks[0] < 1 ? 1 : 0);
  const signed = (v) => (v > 0 ? '+' : v < 0 ? '−' : '') + num(Math.abs(v), digits);
  // Values level with their gridline in a left margin, as in Chart.svelte:
  // the two charts of a run detail line up, refill bands included.
  const ownGutter = $derived(axisGutter(scale.ticks.map(signed)));
  $effect(() => {
    gutter = ownGutter;
  });
  const padL = $derived(Math.max(ownGutter, minGutter));
  const x = (t) => padL + (Math.min(Math.max(t, 0), span) / span) * (W - padL);
  const y = (v) => padT + ((scale.hi - v) / (2 * scale.hi)) * (H - padT - padB);

  const parts = $derived(
    splitEstimated(series, spans).map((p) => ({
      est: p.est,
      d: p.pts.map(([t, v], i) => `${i ? 'L' : 'M'}${x(t).toFixed(1)},${y(v).toFixed(1)}`).join(' '),
    })),
  );
  // none given: the experiment's elapsed time, min then h, as every chart
  const tickMarks = $derived((ticks ?? elapsedTicks(span, ticksFor(W))).filter((k) => k.t >= 0 && k.t <= span).map((k) => ({ ...k, x: x(k.t) })));
</script>

<div class="wrap" bind:clientWidth={boxW}>
  <svg viewBox="0 0 {W} {H}" role="img" aria-label="Delivered minus requested volume since the start, mL">
    {#each spans as [a, b]}
      <rect x={x(a)} y={padT} width={Math.max(2, x(b) - x(a))} height={H - padT - padB} class="band" />
    {/each}
    {#each scale.ticks as v}
      <line x1={padL} x2={W} y1={y(v)} y2={y(v)} class={v === 0 ? 'zero' : 'grid'} />
      <text x={padL - 4} y={y(v) + 3.5} text-anchor="end" class="axl tick">{signed(v)}</text>
    {/each}
    {#each parts as p}
      <path d={p.d} class={p.est ? 'blind' : 'gap'} />
    {/each}
    {#each tickMarks as k, i}
      <text x={k.x} y={H - 6} class="axl"
            text-anchor={i === tickMarks.length - 1 && k.x > W - 30 ? 'end' : k.x < 20 ? 'start' : 'middle'}>{k.label}</text>
    {/each}
  </svg>
</div>

<style>
  .wrap { overflow-x: hidden; }
  svg { width: 100%; height: auto; display: block; }
  .axl { font-family: var(--mono); font-size: 10.5px; fill: var(--muted); }
  .tick { font-size: 9.5px; opacity: 0.8; }
  .grid { stroke: var(--line-soft); stroke-width: 1; }
  .zero { stroke: var(--teal-400); stroke-width: 1.5; }
  .band { fill: var(--fill); fill-opacity: 0.16; }
  .gap { fill: none; stroke: var(--green-500); stroke-width: 1.5; stroke-linejoin: round; }
  .blind { fill: none; stroke: var(--muted); stroke-width: 1.5; stroke-dasharray: 4 4; stroke-linecap: round; }
</style>
