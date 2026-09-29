<script>
  // Delivered (weighed) vs requested feed volume for one run, with R², the
  // cumulative deficit and, for an exponential curve, requested vs fitted µ.
  // Renders nothing for a run without balance data (the API answers 404).
  import { get } from '../lib/api.js';
  import { num } from '../lib/fmt.js';
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
</script>

{#if report}
  <div class="track">
    <div class="eyebrow">Feed delivered vs requested (weighed)</div>
    <Chart planned={requested} actual={delivered} nowS={null} {durationS} unit="mL" digits={1} />
    <div class="stats mono">
      <span>R² <b>{report.r_squared != null ? report.r_squared.toFixed(5) : '·'}</b></span>
      <span>
        deficit <b>{num(report.deficit_ml, 1)} mL</b>{report.deficit_pct != null
          ? ` (${num(report.deficit_pct, 2)} %)`
          : ''}
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
  .stats {
    display: flex;
    flex-wrap: wrap;
    gap: var(--s-2) var(--s-5);
    font-size: 13px;
    margin-top: var(--s-3);
  }
</style>
