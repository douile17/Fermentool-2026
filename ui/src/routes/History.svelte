<script>
  import { get, post } from '../lib/api.js';
  import { num, dur, shortTime, unitFor } from '../lib/fmt.js';
  import Chart from '../components/Chart.svelte';

  let runs = $state([]);
  let err = $state(null);
  let sel = $state(null); // { run, planned, ticks, events }

  $effect(() => {
    get('/api/runs?limit=100')
      .then((r) => (runs = r))
      .catch((e) => (err = e.message));
  });

  async function open(id) {
    sel = null;
    try {
      const run = await get(`/api/runs/${id}`);
      const p = await post('/api/preview', { curve: run.curve, samples: 200 });
      const ticks = await get(`/api/runs/${id}/ticks`);
      const events = await get(`/api/runs/${id}/events?limit=100`);
      sel = { run, planned: p.series, ticks, events };
    } catch (e) {
      err = e.message;
    }
  }

  function exportCsv() {
    if (!sel) return;
    const rows = [['seq', 'wall_time', 'elapsed_s', 'target', 'written_ok', 'readback']];
    for (const t of sel.ticks) {
      rows.push([t.seq, t.wall_time, t.elapsed_s, t.target, t.written_ok ? 1 : 0, t.readback ?? '']);
    }
    const csv = rows.map((r) => r.join(',')).join('\n');
    const url = URL.createObjectURL(new Blob([csv], { type: 'text/csv' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = `${sel.run.name}-run${sel.run.id}.csv`;
    a.click();
    URL.revokeObjectURL(url);
  }
</script>

<section class="card">
  <div class="card-head"><div><div class="eyebrow">History</div><h2>Runs</h2></div></div>
  {#if err}<div class="err">{err}</div>{/if}

  <div class="tbl">
    <div class="tr th">
      <span>Name</span><span>Status</span><span>Curve</span><span>Duration</span><span>Started</span>
    </div>
    {#each runs as r (r.id)}
      <button class="tr" class:sel={sel?.run.id === r.id} onclick={() => open(r.id)}>
        <span class="nm">{r.name}</span>
        <span><span class="pill {r.status}">{r.status}</span></span>
        <span class="mono">{r.curve.params.kind}</span>
        <span class="mono">{dur(r.duration_s)}</span>
        <span class="mono">{shortTime(r.started_at)}</span>
      </button>
    {:else}
      <p class="muted">No runs yet.</p>
    {/each}
  </div>
</section>

{#if sel}
  <section class="card">
    <div class="card-head">
      <div>
        <div class="eyebrow">Run #{sel.run.id} · {unitFor(sel.run.control_var)}</div>
        <h2>{sel.run.name}</h2>
      </div>
      <div class="hd-actions">
        <span class="pill {sel.run.status}">{sel.run.status}</span>
        <button class="btn-ghost" onclick={exportCsv}>Export CSV</button>
      </div>
    </div>

    <Chart
      planned={sel.planned}
      actual={sel.ticks.map((t) => [t.elapsed_s, t.target])}
      nowS={null}
      durationS={sel.run.duration_s}
      unit={unitFor(sel.run.control_var)}
    />
    <div class="pv-cap mono">
      {sel.ticks.length} ticks · start {num(sel.run.curve.start)} → target {num(sel.run.curve.end)}
    </div>

    <div class="acts">
      {#each sel.events as e (e.id)}
        <div class="act">
          <time class="mono">{shortTime(e.wall_time)}</time>
          <span class="pill {e.level}">{e.kind}</span>
          <span class="body">{e.detail ?? ''}</span>
        </div>
      {/each}
    </div>
  </section>
{/if}

<style>
  .tbl { display: flex; flex-direction: column; }
  .tr {
    display: grid;
    grid-template-columns: 1.6fr 0.9fr 1fr 0.9fr 0.8fr;
    gap: var(--s-3);
    align-items: center;
    text-align: left;
    padding: var(--s-3) var(--s-2);
    background: transparent;
    border: none;
    border-radius: var(--radius-ctl);
    font: inherit;
    color: var(--ink);
    cursor: pointer;
  }
  .tr.th { color: var(--muted); font-size: 11px; letter-spacing: 0.08em; text-transform: uppercase; cursor: default; }
  .tr:not(.th):hover { background: var(--surface-sunken); }
  .tr.sel { background: color-mix(in srgb, var(--teal-700) 9%, var(--surface)); }
  .tr .nm { font-weight: 600; }
  .tr span { font-size: 13px; }
  .muted { color: var(--muted); padding: var(--s-3) var(--s-2); }

  .hd-actions { display: flex; align-items: center; gap: var(--s-3); }
  .pv-cap { font-size: 12px; color: var(--muted); margin-top: var(--s-2); }

  .acts { margin-top: var(--s-5); display: flex; flex-direction: column; }
  .act {
    display: grid; grid-template-columns: 60px 96px 1fr; gap: var(--s-3);
    align-items: baseline; padding: var(--s-2) 0; font-size: 13px;
  }
  .act + .act { border-top: 1px solid var(--line-soft); }
  .act time { color: var(--muted); font-size: 12px; }
  .act .body { color: var(--muted); }

  @media (max-width: 720px) {
    .tr { grid-template-columns: 1.4fr 0.9fr 0.9fr; }
    .tr span:nth-child(4), .tr span:nth-child(5) { display: none; }
  }
</style>
