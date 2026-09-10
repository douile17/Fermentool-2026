<script>
  import { app } from '../lib/state.svelte.js';
  import { get, post, del } from '../lib/api.js';
  import { num, dur, shortTime, stampY, unitFor, digitsFor } from '../lib/fmt.js';
  import Chart from '../components/Chart.svelte';

  let runs = $state([]);
  let err = $state(null);
  let sel = $state(null); // { run, planned, ticks, events }
  let q = $state('');

  let confirmClear = $state(false);
  let clearing = $state(false);
  let clearErr = $state(null);
  // The daemon refuses this while a run is active OR the pump is still holding
  // a completed run's setpoint; guard client-side too.
  const clearBlocked = $derived(!!app.status?.active || !!app.status?.holding);

  async function clearHistory() {
    clearing = true;
    clearErr = null;
    try {
      await del('/api/history');
      runs = [];
      sel = null;
      q = '';
      confirmClear = false;
    } catch (e) {
      clearErr = e.message;
    }
    clearing = false;
  }

  const shown = $derived.by(() => {
    const needle = q.trim().toLowerCase();
    return needle ? runs.filter((r) => (r.name ?? '').toLowerCase().includes(needle)) : runs;
  });

  $effect(() => {
    get('/api/runs?limit=100')
      .then((r) => (runs = r))
      .catch((e) => (err = e.message));
  });

  // Clicking a row opens its read-only detail - except a still-running run,
  // which jumps to the live Overview instead.
  function pick(r) {
    if (r.status === 'running') {
      app.tab = 'overview';
      return;
    }
    open(r.id);
  }

  // The server caps one /ticks response at 50 000 rows (seq window), so a run
  // longer than ~14 h needs several pages. Walk seq from 0 until a window comes
  // back that doesn't reach its own end - that's the last of the ticks.
  async function fetchAllTicks(id) {
    const SPAN = 50000; // must match the daemon's MAX_TICKS_SPAN
    const all = [];
    for (let from = 0; ; ) {
      const batch = await get(`/api/runs/${id}/ticks?from=${from}&to=${from + SPAN}`);
      if (!batch.length) break;
      all.push(...batch);
      const lastSeq = batch[batch.length - 1].seq;
      if (lastSeq < from + SPAN) break;
      from = lastSeq + 1;
    }
    return all;
  }

  async function open(id) {
    sel = null;
    try {
      const run = await get(`/api/runs/${id}`);
      const p = await post('/api/preview', { curve: run.curve, samples: 200 });
      const ticks = await fetchAllTicks(id);
      const events = await get(`/api/runs/${id}/events?limit=100`);
      sel = { run, planned: p.series, ticks, events };
    } catch (e) {
      err = e.message;
    }
  }

  // Seed NewRun from a past run's options and jump there.
  function runAgain() {
    if (!sel) return;
    const { run } = sel;
    const c = run.curve;
    const s = Math.max(0, Math.round(run.duration_s));
    app.prefill = {
      name: run.name,
      control_var: run.control_var,
      direction: run.direction,
      duration_h: Math.floor(s / 3600),
      duration_m: Math.floor((s % 3600) / 60),
      duration_s: s % 60,
      kind: c.params.kind,
      mode: c.mode === 'physio' ? 'physio' : 'endpoints',
      start: c.start,
      end: c.end,
      value: c.start,
      mu_per_hour: c.params.mu_per_hour ?? 0.15,
      steepness: c.params.steepness ?? 8,
    };
    app.tab = 'new';
  }

  // A 100 h run journals ~360k ticks; drawing them all is a 360k-segment SVG
  // path that locks the tab. Downsample to a few thousand for the chart only -
  // the CSV export still uses the full set.
  const chartActual = $derived.by(() => {
    if (!sel) return [];
    const t = sel.ticks;
    const cap = 3000;
    if (t.length <= cap) return t.map((p) => [p.elapsed_s, p.target]);
    const stride = Math.ceil(t.length / cap);
    const out = [];
    for (let i = 0; i < t.length; i += stride) out.push([t[i].elapsed_s, t[i].target]);
    const last = t[t.length - 1];
    out.push([last.elapsed_s, last.target]);
    return out;
  });

  function closeSel() {
    sel = null;
  }
  function onKey(e) {
    if (e.key !== 'Escape') return;
    if (confirmClear && !clearing) confirmClear = false;
    else if (sel) closeSel();
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

<svelte:window on:keydown={onKey} />

<section class="card">
  <div class="card-head">
    <div><div class="eyebrow">History</div><h2>Runs</h2></div>
    <div class="hd-tools">
      <input
        class="search"
        type="search"
        placeholder="Search by name…"
        bind:value={q}
        aria-label="Search runs by name"
      />
      <button
        class="btn-ghost reset"
        onclick={() => { clearErr = null; confirmClear = true; }}
        disabled={!runs.length || clearBlocked}
        title={clearBlocked ? 'Stop the run / pump first' : 'Delete every run'}
      >
        Reset history
      </button>
    </div>
  </div>
  {#if err}<div class="err">{err}</div>{/if}

  <div class="tbl">
    <div class="tr th">
      <span>Name</span><span>Status</span><span>Curve</span><span>Duration</span><span>Started</span>
    </div>
    {#each shown as r (r.id)}
      <button
        class="tr"
        class:sel={sel?.run.id === r.id}
        onclick={() => pick(r)}
        title={r.status === 'running' ? 'View live in Overview' : 'Open details'}
      >
        <span class="nm">{r.name}</span>
        <span><span class="pill {r.status}">{r.status}</span></span>
        <span class="mono">{r.curve.params.kind}</span>
        <span class="mono">{dur(r.duration_s)}</span>
        <span class="mono">{stampY(r.started_at)}</span>
      </button>
    {:else}
      <p class="muted">{q.trim() ? `No run matches “${q.trim()}”.` : 'No runs yet.'}</p>
    {/each}
  </div>
</section>

{#if confirmClear}
  <button class="backdrop" aria-label="Cancel" onclick={() => (confirmClear = false)}></button>
  <div class="layer">
    <div class="modal card confirm" role="alertdialog" aria-modal="true" aria-labelledby="ft-clear">
      <div class="eyebrow">Reset history</div>
      <h2 id="ft-clear">Delete all {runs.length} run{runs.length === 1 ? '' : 's'}?</h2>
      <p class="lede">
        Every run, its journal and its events are permanently removed. This can't be undone.
      </p>
      {#if clearErr}<div class="err">{clearErr}</div>{/if}
      <div class="modal-foot">
        <button class="btn-danger" disabled={clearing} onclick={clearHistory}>
          {clearing ? 'Deleting…' : 'Delete everything'}
        </button>
        <button class="btn-ghost" disabled={clearing} onclick={() => (confirmClear = false)}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

{#if sel}
  <button class="backdrop" aria-label="Close" onclick={closeSel}></button>
  <div class="layer">
    <div class="modal card" role="dialog" aria-modal="true" aria-labelledby="ft-run-detail">
      <div class="card-head">
        <div>
          <div class="eyebrow">Run #{sel.run.id} · {unitFor(sel.run.control_var)}</div>
          <h2 id="ft-run-detail">{sel.run.name}</h2>
          <div class="sub mono">
            {stampY(sel.run.started_at)}{sel.run.ended_at ? ` → ${stampY(sel.run.ended_at)}` : ''}
          </div>
        </div>
        <div class="hd-actions">
          <span class="pill {sel.run.status}">{sel.run.status}</span>
          <button class="btn-ghost" onclick={exportCsv}>Export CSV</button>
        </div>
      </div>

      <Chart
        planned={sel.planned}
        actual={chartActual}
        nowS={null}
        durationS={sel.run.duration_s}
        unit={unitFor(sel.run.control_var)}
        digits={digitsFor(sel.run.control_var)}
      />
      <div class="pv-cap mono">
        {dur(sel.run.duration_s)} · {sel.run.curve.params.kind} ·
        {num(sel.run.curve.start, digitsFor(sel.run.control_var))} → {num(sel.run.curve.end, digitsFor(sel.run.control_var))}
        {unitFor(sel.run.control_var)}
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

      <div class="modal-foot">
        <button class="btn-primary" onclick={runAgain}>Run again</button>
        <button class="btn-ghost" onclick={closeSel}>Close</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .tbl { display: flex; flex-direction: column; }
  .tr {
    display: grid;
    grid-template-columns: 1.5fr 0.8fr 0.9fr 0.8fr 1.2fr;
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
  .sub { font-size: 12px; color: var(--muted); margin-top: 3px; }

  .search {
    align-self: center;
    width: min(260px, 42vw);
    padding: var(--s-2) var(--s-3);
    font: inherit;
    font-size: 13px;
    color: var(--ink);
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius-ctl);
  }
  .search:focus-visible { outline: 2px solid color-mix(in srgb, var(--teal-700) 45%, transparent); outline-offset: 1px; }

  .hd-tools { display: flex; align-items: center; gap: var(--s-3); flex-wrap: wrap; }
  .reset {
    align-self: center;
    font-size: 13px;
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 35%, var(--line));
  }
  .reset:not(:disabled):hover { background: var(--danger-bg); }

  .confirm { max-width: 420px; text-align: left; }
  .confirm h2 { font-size: 17px; margin: 6px 0 var(--s-3); }
  .confirm .lede { margin: 0; color: var(--muted); font-size: 13px; }
  .confirm .err { margin-top: var(--s-3); }
  .pv-cap {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
    margin-top: var(--s-3);
    padding: var(--s-2) var(--s-3);
    background: var(--surface-sunken);
    border-radius: var(--radius-ctl);
  }

  .backdrop {
    position: fixed; inset: 0; border: none; padding: 0;
    background: rgba(18, 41, 46, 0.28); cursor: default; z-index: 60;
  }
  .layer {
    position: fixed; inset: 0; display: grid; place-items: center;
    padding: var(--s-5); z-index: 61; pointer-events: none;
  }
  .modal {
    width: 100%; max-width: 720px; max-height: 85vh; overflow-y: auto;
    box-shadow: var(--shadow-pop); pointer-events: auto;
    animation: pop 0.18s ease-out;
  }
  @keyframes pop {
    from { transform: scale(0.97); opacity: 0; }
    to { transform: scale(1); opacity: 1; }
  }
  .modal-foot {
    display: flex; gap: var(--s-3); justify-content: flex-end;
    flex-wrap: wrap; margin-top: var(--s-5);
  }
  @media (prefers-reduced-motion: reduce) { .modal { animation: none; } }

  .acts { margin-top: var(--s-4); display: flex; flex-direction: column; }
  .act {
    display: grid; grid-template-columns: 60px 96px 1fr; gap: var(--s-3);
    align-items: baseline; padding: var(--s-2) 0; font-size: 12px;
    color: var(--muted);
  }
  .act + .act { border-top: 1px solid var(--line-soft); }
  .act time { color: var(--muted); font-size: 12px; }
  .act .body { color: var(--muted); }

  @media (max-width: 720px) {
    .tr { grid-template-columns: 1.4fr 0.9fr 0.9fr; }
    .tr span:nth-child(4), .tr span:nth-child(5) { display: none; }
  }
</style>
