<script>
  // One calibration measurement on the balance, every second: grams
  // collected since its start, the slope line over the steady middle, and
  // the start and end transients the slope leaves out, shaded.
  import { valueScale } from '../lib/chartscale.js';
  import { num } from '../lib/fmt.js';
  import { SKIP_START_S, SKIP_END_S } from '../lib/measurement.js';

  /** @type {{ profile: { pts: [number, number][], fit: { k: number, b0: number }, endS: number }, label?: string }} */
  let { profile, label = 'Weight collected during the measurement' } = $props();

  let boxW = $state(640);
  const W = $derived(Math.max(240, Math.round(boxW)));
  const H = 170;
  const padT = 10;
  const padB = 22;

  const end = $derived(profile.endS > 0 ? profile.endS : 1);
  const scale = $derived(valueScale(profile.pts.map((p) => p[1])));
  const x = (t) => (Math.max(0, Math.min(t, end)) / end) * W;
  const y = (v) => padT + ((scale.hi - v) / (scale.hi - scale.lo || 1)) * (H - padT - padB);
  const tickDigits = $derived(scale.ticks.length > 1 && scale.ticks[1] - scale.ticks[0] < 1 ? 1 : 0);

  const line = $derived(profile.pts.map(([t, g], i) => `${i ? 'L' : 'M'}${x(t).toFixed(1)},${y(g).toFixed(1)}`).join(' '));
  const fitFrom = SKIP_START_S;
  const fitTo = $derived(end - SKIP_END_S);
  // a time label every minute, or every 30 s under 2 min
  const timeTicks = $derived.by(() => {
    const step = end <= 120 ? 30 : end <= 600 ? 60 : end <= 1800 ? 300 : 600;
    const out = [];
    for (let t = 0; t <= end + 1e-9; t += step) out.push(t);
    return out;
  });
  const mmss = (t) => `${Math.floor(t / 60)}:${String(Math.round(t % 60)).padStart(2, '0')}`;
</script>

<div class="wrap" bind:clientWidth={boxW}>
  <svg viewBox="0 0 {W} {H}" role="img" aria-label={label}>
    <rect x={x(0)} y={padT} width={x(fitFrom) - x(0)} height={H - padT - padB} class="skip" />
    <rect x={x(fitTo)} y={padT} width={Math.max(0, x(end) - x(fitTo))} height={H - padT - padB} class="skip" />
    {#each scale.ticks as v}
      <line x1="0" x2={W} y1={y(v)} y2={y(v)} class="grid" />
      <text x="2" y={Math.max(10, y(v) - 3)} class="axl tick">{num(v, tickDigits)}</text>
    {/each}
    {#each timeTicks as t, i}
      <text x={x(t)} y={H - 6} class="axl"
            text-anchor={i === 0 ? 'start' : i === timeTicks.length - 1 && x(t) > W - 30 ? 'end' : 'middle'}>{mmss(t)}</text>
    {/each}
    <path class="fit" d="M{x(fitFrom)},{y(profile.fit.b0 + profile.fit.k * fitFrom)} L{x(fitTo)},{y(profile.fit.b0 + profile.fit.k * fitTo)}" />
    <path class="w" d={line} />
  </svg>
</div>

<style>
  .wrap { overflow-x: hidden; }
  svg { width: 100%; height: auto; display: block; }
  .axl { font-family: var(--mono); font-size: 10.5px; fill: var(--muted); }
  .tick { font-size: 9.5px; opacity: 0.8; }
  .grid { stroke: var(--line-soft); stroke-width: 1; }
  .skip { fill: var(--surface-sunken); }
  .w { fill: none; stroke: var(--green-500); stroke-width: 2; stroke-linejoin: round; }
  .fit { fill: none; stroke: var(--ink); stroke-opacity: 0.55; stroke-width: 1.25; stroke-dasharray: 4 3; }
</style>
