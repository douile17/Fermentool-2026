<script>
  import { app } from '../lib/state.svelte.js';
  import { get, post, del } from '../lib/api.js';
  import { num, dur, shortTime, stampY, unitFor, digitsFor } from '../lib/fmt.js';
  import Chart from '../components/Chart.svelte';
  import TrackingPanel from '../components/TrackingPanel.svelte';
  import Icon from '../components/Icon.svelte';
  import { bottleWeights } from '../lib/balance.js';
  import { downloadSettings } from '../lib/runfile.js';

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

  // Calibration bursts are runs in the database (Tubing calibration records
  // which ones it used) but not feeding cycles: never listed here.
  const shown = $derived.by(() => {
    const needle = q.trim().toLowerCase();
    const kept = runs.filter((r) => r.kind !== 'calibration');
    return needle ? kept.filter((r) => (r.name ?? '').toLowerCase().includes(needle)) : kept;
  });

  // Several runs ticked in the list, deleted together. A running run can't
  // be ticked; ids that leave the list (search, reload) leave the selection.
  let picked = $state([]);
  const pickable = $derived(shown.filter((r) => r.status !== 'running'));
  const pickedShown = $derived(picked.filter((id) => shown.some((r) => r.id === id)));
  const allPicked = $derived(pickable.length > 0 && pickable.every((r) => picked.includes(r.id)));
  function togglePick(id) {
    picked = picked.includes(id) ? picked.filter((x) => x !== id) : [...picked, id];
  }
  function toggleAll() {
    const ids = pickable.map((r) => r.id);
    picked = allPicked ? picked.filter((x) => !ids.includes(x)) : [...new Set([...picked, ...ids])];
  }

  let confirmBulk = $state(false);
  let bulkBusy = $state(false);
  let bulkErr = $state(null);
  async function deletePicked() {
    bulkBusy = true;
    bulkErr = null;
    const failed = [];
    for (const id of pickedShown) {
      try {
        await del(`/api/runs/${id}`);
        runs = runs.filter((r) => r.id !== id);
        picked = picked.filter((x) => x !== id);
      } catch (e) {
        const name = runs.find((r) => r.id === id)?.name ?? `#${id}`;
        failed.push(`${name}: ${e.message}`);
      }
    }
    bulkBusy = false;
    if (failed.length) bulkErr = failed.join('\n');
    else confirmBulk = false;
  }

  $effect(() => {
    get('/api/runs?limit=100000')
      .then((r) => (runs = r))
      .catch((e) => (err = e.message));
  });

  // Clicking a row opens its read-only detail, except a still-running run,
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
  // back that doesn't reach its own end, that's the last of the ticks.
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
      // A sample for the chart: the full journal (up to ~360k rows for a
      // 100 h run) is only fetched for the CSV export.
      const ticks = await get(`/api/runs/${id}/ticks?sample=3000`);
      const events = await get(`/api/runs/${id}/events?limit=100`);
      // Runs without the balance trim have no report (404): no summary then.
      const report = await get(`/api/runs/${id}/tracking`).catch(() => null);
      sel = { run, planned: p.series, ticks, events, report };
    } catch (e) {
      err = e.message;
    }
  }

  // A past run's options as New run form fields: for "Run again" and for
  // the settings file.
  function settingsOf(run) {
    const c = run.curve;
    const s = Math.max(0, Math.round(run.duration_s));
    return {
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
      gravimetric_trim: !!run.gravimetric_trim,
      // New run drops it if it no longer fits (an ml/min calibration needs the trim).
      tubing_calibration_id: run.tubing_calibration_id ?? null,
      ...(run.responsible ? { responsible: run.responsible } : {}),
    };
  }

  // The curve shapes the New run form can rebuild. A step or custom curve
  // (made through the API) would come back as a constant one.
  const FORM_KINDS = ['linear', 'exponential', 'sigmoid', 'constant'];
  const replayable = $derived(!!sel && FORM_KINDS.includes(sel.run.curve.params.kind));
  const notReplayable = 'New run cannot rebuild this curve shape';

  // Seed NewRun from a past run's options and jump there.
  function runAgain() {
    if (!sel || !replayable) return;
    app.prefill = settingsOf(sel.run);
    app.tab = 'new';
  }

  // One run, its journal and events, removed for good (confirmed first).
  let confirmDelete = $state(false);
  let deleting = $state(false);
  let deleteErr = $state(null);
  async function deleteRun() {
    if (!sel) return;
    const id = sel.run.id;
    deleting = true;
    deleteErr = null;
    try {
      await del(`/api/runs/${id}`);
      runs = runs.filter((r) => r.id !== id);
      confirmDelete = false;
      closeSel();
    } catch (e) {
      deleteErr = e.message;
    }
    deleting = false;
  }

  // The detail holds a server-side sample (a few thousand ticks); thinned
  // again here only if a caller ever hands it more.
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

  const warnCount = $derived(
    sel ? sel.events.filter((e) => e.level === 'warn' || e.level === 'error').length : 0,
  );

  function closeSel() {
    sel = null;
    confirmDelete = false;
  }
  function onKey(e) {
    if (e.key !== 'Escape') return;
    if (confirmDelete) {
      if (!deleting) confirmDelete = false;
    } else if (confirmBulk && !bulkBusy) confirmBulk = false;
    else if (confirmClear && !clearing) confirmClear = false;
    else if (sel) closeSel();
  }

  // A summary block (run, bottle weights, totals), a blank line, then one row
  // per journalled second with the balance reading and the delivered total.
  let exporting = $state(false);
  async function exportCsv() {
    if (!sel || exporting) return;
    const { run, report } = sel;
    exporting = true;
    let ticks;
    try {
      ticks = await fetchAllTicks(run.id);
    } catch (e) {
      err = e.message;
      exporting = false;
      return;
    }
    exporting = false;
    const cell = (v) => {
      const s = v == null ? '' : String(v);
      return /[",\n]/.test(s) ? `"${s.replaceAll('"', '""')}"` : s;
    };
    const fix = (v, d) => (v == null ? '' : Number(v).toFixed(d));
    const rows = [
      ['run_id', run.id],
      ['name', run.name],
      ['status', run.status],
      ['started_at', run.started_at],
      ['ended_at', run.ended_at ?? ''],
      ['responsible', run.responsible ?? ''],
      ['control_var', run.control_var],
      ['balance_trim', run.gravimetric_trim ? 1 : 0],
    ];
    const bottle = bottleWeights(report);
    if (bottle) {
      rows.push(['balance_start_g', fix(bottle.start, 1)]);
      bottle.refills.forEach((r, i) => {
        rows.push([`refill_${i + 1}_elapsed_s`, fix(r.t_s, 0)]);
        rows.push([`refill_${i + 1}_before_g`, fix(r.before_g, 1)]);
        rows.push([`refill_${i + 1}_after_g`, fix(r.after_g, 1)]);
      });
      rows.push(['balance_end_g', fix(bottle.end, 1)]);
      rows.push(['weighed_out_g', fix(bottle.weighedOut, 1)]);
    }
    const last = report?.points?.at(-1);
    if (last) {
      rows.push(['requested_ml', fix(last[1], 2)]);
      rows.push(['delivered_ml', fix(last[2], 2)]);
      rows.push(['gap_ml', fix(last[2] - last[1], 2)]);
    }
    rows.push([]);
    rows.push(['seq', 'wall_time', 'elapsed_s', 'target', 'written_ok', 'readback', 'balance_g', 'delivered_g']);
    for (const t of ticks) {
      rows.push([
        t.seq, t.wall_time, t.elapsed_s, t.target, t.written_ok ? 1 : 0, t.readback ?? '',
        t.weight_g ?? '', t.delivered_g ?? '',
      ]);
    }
    const csv = rows.map((r) => r.map(cell).join(',')).join('\n') + '\n';
    const url = URL.createObjectURL(new Blob([csv], { type: 'text/csv' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = `${run.name}-run${run.id}.csv`;
    a.click();
    // Not at once: some browsers drop a download whose URL is gone before it starts.
    setTimeout(() => URL.revokeObjectURL(url), 10_000);
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
      {#if pickedShown.length}
        <button class="btn-danger" onclick={() => { bulkErr = null; confirmBulk = true; }}>
          Delete selected ({pickedShown.length})
        </button>
      {/if}
      <button
        class="btn-danger"
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
    <div class="row">
      <label class="pick" title={allPicked ? 'Unselect all' : 'Select all'}>
        <input type="checkbox" checked={allPicked} disabled={!pickable.length} onchange={toggleAll} aria-label="Select all runs" />
      </label>
      <div class="tr th">
        <span>Name</span><span>Status</span><span>Curve</span><span>Duration</span><span>Started</span>
      </div>
    </div>
    {#each shown as r (r.id)}
      <div class="row">
        <label class="pick">
          <input type="checkbox" checked={picked.includes(r.id)} disabled={r.status === 'running'}
            onchange={() => togglePick(r.id)} aria-label="Select {r.name}" />
        </label>
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
      </div>
    {:else}
      <p class="muted">{q.trim() ? `No run matches “${q.trim()}”.` : 'No runs yet.'}</p>
    {/each}
  </div>
</section>

{#if confirmBulk}
  <button class="backdrop" aria-label="Cancel" onclick={() => !bulkBusy && (confirmBulk = false)}></button>
  <div class="layer">
    <div class="modal card confirm" role="alertdialog" aria-modal="true" aria-labelledby="ft-bulk">
      <div class="eyebrow">Delete runs</div>
      <h2 id="ft-bulk">Delete {pickedShown.length} run{pickedShown.length === 1 ? '' : 's'}?</h2>
      <p class="lede">
        The selected runs, their journal and their events are permanently removed. This can't be undone.
      </p>
      {#if bulkErr}<div class="err bulk-err">{bulkErr}</div>{/if}
      <div class="modal-foot">
        <button class="btn-danger" disabled={bulkBusy || !pickedShown.length} onclick={deletePicked}>
          {bulkBusy ? 'Deleting…' : 'Delete'}
        </button>
        <button class="btn-ghost" disabled={bulkBusy} onclick={() => (confirmBulk = false)}>
          {bulkErr ? 'Close' : 'Cancel'}
        </button>
      </div>
    </div>
  </div>
{/if}

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
    <div class="modal card detail" role="dialog" aria-modal="true" aria-labelledby="ft-run-detail">
      <!-- Sticky head: the run's identity and its main actions stay in reach
           however far the journal below is scrolled. -->
      <div class="card-head detail-head">
        <div>
          <span class="pill {sel.run.status}">{sel.run.status}</span>
          <div class="eyebrow id-line">Run #{sel.run.id} | {unitFor(sel.run.control_var)}</div>
          <h2 id="ft-run-detail">{sel.run.name}</h2>
          <div class="sub mono">
            {stampY(sel.run.started_at)}{sel.run.ended_at ? ` → ${stampY(sel.run.ended_at)}` : ''}
            {sel.run.responsible ? ` | ${sel.run.responsible}` : ''}
          </div>
        </div>
        <div class="hd-actions">
          <button class="btn-ghost" disabled={exporting} onclick={exportCsv}
            title="Download the run's journal (one row per second)">
            {exporting ? 'Exporting…' : 'Export CSV'}
          </button>
          <button class="btn-primary" disabled={!replayable} onclick={runAgain}
            title={replayable ? "Open New run with this run's settings" : notReplayable}>
            Run again
          </button>
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
        {dur(sel.run.duration_s)} | {sel.run.curve.params.kind} |
        {num(sel.run.curve.start, digitsFor(sel.run.control_var))} → {num(sel.run.curve.end, digitsFor(sel.run.control_var))}
        {unitFor(sel.run.control_var)}
      </div>

      <!-- Detail sections start folded: the chart answers "how did it go",
           the regulation and the journal are there for when it went wrong. -->
      <TrackingPanel runId={sel.run.id} durationS={sel.run.duration_s} collapsible />

      {#if sel.events.length}
        <details class="fold">
          <summary>
            <Icon name="chevron-right" size={16} />
            <span class="eyebrow">Events</span>
            <span class="peek mono">
              {sel.events.length}{sel.events.length >= 100 ? '+' : ''}
              {#if warnCount}| <b class="off">{warnCount} warning{warnCount === 1 ? '' : 's'}</b>{/if}
            </span>
          </summary>
          <div class="acts">
            {#each sel.events as e (e.id)}
              <div class="act">
                <time class="mono">{shortTime(e.wall_time)}</time>
                <span class="pill {e.level}">{e.kind}</span>
                <span class="body">{e.detail ?? ''}</span>
              </div>
            {/each}
          </div>
        </details>
      {/if}

      <div class="modal-foot">
        <button class="btn-danger del" disabled={sel.run.status === 'running'} onclick={() => { deleteErr = null; confirmDelete = true; }}>
          Delete run
        </button>
        <button class="btn-ghost" disabled={!replayable} onclick={() => downloadSettings(settingsOf(sel.run))}
          title={replayable ? "Save this run's settings to a file, to import later in New run" : notReplayable}>
          Export settings
        </button>
        <button class="btn-ghost" onclick={closeSel}>Close</button>
      </div>
    </div>
  </div>

  <!-- Confirmed in its own small dialog over the detail, like the bulk
       delete: the detail's footer never reflows under the cursor. -->
  {#if confirmDelete}
    <button class="backdrop over" aria-label="Cancel" onclick={() => !deleting && (confirmDelete = false)}></button>
    <div class="layer over">
      <div class="modal card confirm" role="alertdialog" aria-modal="true" aria-labelledby="ft-del-one">
        <div class="eyebrow">Delete run</div>
        <h2 id="ft-del-one">Delete “{sel.run.name}”?</h2>
        <p class="lede">
          Run #{sel.run.id}, its journal and its events are permanently removed. This can't be undone.
        </p>
        {#if deleteErr}<div class="err">{deleteErr}</div>{/if}
        <div class="modal-foot">
          <button class="btn-danger" disabled={deleting} onclick={deleteRun}>
            {deleting ? 'Deleting…' : 'Delete'}
          </button>
          <button class="btn-ghost" disabled={deleting} onclick={() => (confirmDelete = false)}>Cancel</button>
        </div>
      </div>
    </div>
  {/if}
{/if}

<style>
  .tbl { display: flex; flex-direction: column; }
  .row { display: flex; align-items: center; gap: var(--s-1); }
  .row .tr { flex: 1; min-width: 0; }
  .pick { display: flex; align-items: center; justify-content: center; width: 28px; flex: none; cursor: pointer; }
  .pick input { margin: 0; cursor: pointer; }
  .pick input:disabled { cursor: default; }
  .bulk-err { white-space: pre-line; margin-top: var(--s-3); }
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

  .hd-actions { display: flex; align-items: center; gap: var(--s-3); flex: none; }
  .id-line { margin-top: var(--s-3); }
  /* No top padding on the detail: the sticky head is the top edge itself. */
  .modal.detail { padding-top: 0; }
  .detail-head { align-items: center;
    position: sticky; top: 0; z-index: 2;
    margin: 0 calc(var(--s-7) * -1) var(--s-5);
    padding: var(--s-7) var(--s-7) var(--s-4); /* top = the card's bottom padding */
    background: var(--surface);
    border-bottom: 1px solid var(--line-soft);
  }

  .fold {
    margin-top: var(--s-5); padding-top: var(--s-5);
    border-top: 1px solid color-mix(in srgb, var(--muted) 32%, transparent);
  }
  .fold summary {
    display: flex; align-items: center; gap: var(--s-2);
    cursor: pointer; list-style: none; border-radius: var(--radius-ctl);
  }
  .fold summary::-webkit-details-marker { display: none; }
  .fold summary :global(.icon) { color: var(--muted); transition: transform 0.15s ease; }
  .fold[open] summary :global(.icon) { transform: rotate(90deg); }
  .fold summary .eyebrow { color: var(--ink); }
  .fold summary:focus-visible { outline: 2px solid color-mix(in srgb, var(--teal-700) 45%, transparent); outline-offset: 2px; }
  .peek { margin-left: auto; font-size: 12px; color: var(--muted); }
  .peek .off { color: var(--danger); font-weight: 600; }
  @media (prefers-reduced-motion: reduce) { .fold summary :global(.icon) { transition: none; } }
  .sub { font-size: 12px; color: var(--muted); margin-top: 3px; }

  .search {
    align-self: center;
    width: min(260px, 42vw);
    padding: var(--s-2) var(--s-3);
    font: inherit;
    color: var(--ink);
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius-ctl);
  }
  .search:focus-visible { outline: 2px solid color-mix(in srgb, var(--teal-700) 45%, transparent); outline-offset: 1px; }

  .hd-tools { display: flex; align-items: center; gap: var(--s-3); flex-wrap: wrap; }

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
  .modal-foot .del { margin-right: auto; }
  .backdrop.over { z-index: 62; }
  .layer.over { z-index: 63; }
  @media (prefers-reduced-motion: reduce) { .modal { animation: none; } }

  .acts { margin-top: var(--s-3); display: flex; flex-direction: column; }
  .act {
    display: grid; grid-template-columns: 60px 96px 1fr; gap: var(--s-3);
    align-items: baseline; padding: var(--s-2) 0; font-size: 12px;
    color: var(--muted);
  }
  .act + .act { border-top: 1px solid var(--line-soft); }
  .act time { color: var(--muted); font-size: 12px; }
  .act .body { color: var(--muted); }

  @media (max-width: 720px) {
    /* No top padding on the detail: the sticky head is the top edge itself. */
  .modal.detail { padding-top: 0; }
  .detail-head { align-items: center; flex-wrap: wrap; }
    .tr { grid-template-columns: 1.4fr 0.9fr 0.9fr; }
    .tr span:nth-child(4), .tr span:nth-child(5) { display: none; }
  }
</style>
