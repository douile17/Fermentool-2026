<script>
  import { app } from '../lib/state.svelte.js';
  import { get, post, del } from '../lib/api.js';
  import { num, shortTime, stampY, unitFor, digitsFor, ranFor } from '../lib/fmt.js';
  import Chart from '../components/Chart.svelte';
  import GapChart from '../components/GapChart.svelte';
  import Icon from '../components/Icon.svelte';
  import { bottleWeights } from '../lib/balance.js';
  import { downloadSettings } from '../lib/runfile.js';
  import { weighedFlow, flowWindowS } from '../lib/runview.js';
  import {
    TOLERANCE, alarmCount, atRun, gapExtremes, grouped, journalLines, mins,
    refillRows, regulationFigures, verdict, whenLine, curveChips, blindLegend,
  } from '../lib/rundetail.js';
  import { rateLabel, rateMarks, rateSteps, runAgainSeed } from '../lib/ratechange.js';

  let runs = $state([]);
  let err = $state(null);
  // A running run's "so far" moves on: a minute is plenty for a list.
  let now = $state(Date.now());
  $effect(() => {
    const id = setInterval(() => (now = Date.now()), 60_000);
    return () => clearInterval(id);
  });

  let sel = $state(null); // { run, planned, ticks, events, report, cal }
  // The journal's events fetched for a detail: enough for any run's.
  const EVENTS_MAX = 1000;
  let rawJournal = $state(false);
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
    // by run name or by the person it was for
    const hit = (r) => (r.name ?? '').toLowerCase().includes(needle) || (r.responsible ?? '').toLowerCase().includes(needle);
    return needle ? kept.filter(hit) : kept;
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
      // the curve the run followed, its rate changes included
      const p = await post('/api/preview', { curve: run.effective_curve ?? run.curve, samples: 200 });
      // A sample for the chart: the full journal (up to ~360k rows for a
      // 100 h run) is only fetched for the CSV export.
      const ticks = await get(`/api/runs/${id}/ticks?sample=3000`);
      const events = await get(`/api/runs/${id}/events?limit=${EVENTS_MAX}`);
      // Runs without the balance trim have no report (404): no summary then.
      const report = await get(`/api/runs/${id}/tracking`).catch(() => null);
      // The tube, for Settings: the calibration the run used, if still listed.
      const cal = run.tubing_calibration_id == null
        ? null
        : await get('/api/calibrations')
            .then((cs) => cs.find((c) => c.id === run.tubing_calibration_id) ?? null)
            .catch(() => null);
      rawJournal = false;
      sel = { run, planned: p.series, ticks, events, report, cal };
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
      auto_refill: !!run.auto_refill,
      refill_draw_kg: run.refill_draw_g != null ? run.refill_draw_g / 1000 : null,
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

  // Seed NewRun from a past run's options and jump there. A run whose rate
  // changed asks first from which of its rates.
  const steps = $derived(sel ? rateSteps(sel.run) : []);
  const marks = $derived(sel ? rateMarks(sel.run) : []);
  let pickRate = $state(false);
  let pickedRate = $state(0);
  function runAgain() {
    if (!sel || !replayable) return;
    if (steps.length > 1) {
      pickedRate = steps.length - 1;
      pickRate = true;
      return;
    }
    app.prefill = settingsOf(sel.run);
    app.tab = 'new';
  }
  // The dialog opens on the rate it preselects, as Change rate opens on
  // its field: the arrow keys then move between the rates.
  let ratesEl = $state(null);
  $effect(() => {
    if (pickRate && ratesEl) ratesEl.querySelector('input:checked')?.focus();
  });
  function runAgainAt() {
    if (!sel) return;
    app.prefill = runAgainSeed(settingsOf(sel.run), steps[pickedRate].value);
    pickRate = false;
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
  // The balance's view when the run was weighed: the flow it really got.
  // Otherwise what was sent to the pump, the only record there is.
  const flowWindow = $derived(flowWindowS(sel?.run?.duration_s));
  const chartWeighed = $derived(
    sel?.report?.points?.length ? weighedFlow(sel.report.points, sel.planned, flowWindow) : [],
  );
  const chartActual = $derived.by(() => {
    if (!sel) return [];
    if (chartWeighed.length) return chartWeighed;
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

  const spans = $derived(sel?.report?.estimated ?? []);
  // A run goes on past its curve (the hold): both charts span all of it.
  const reportSpanS = $derived(
    Math.max(sel?.run.duration_s ?? 0, sel?.report?.points?.at(-1)?.[0] ?? 0, sel?.ticks?.at(-1)?.elapsed_s ?? 0),
  );
  const verdictOf = $derived(verdict(sel?.report));
  const alarms = $derived(alarmCount(sel?.events));
  const bottle = $derived(bottleWeights(sel?.report));
  const refills = $derived(refillRows(bottle, spans));
  const nRefills = $derived(sel?.report?.refills?.length ?? 0);
  const journal = $derived(sel ? journalLines(sel.events, sel.run) : []);
  const reg = $derived(regulationFigures(sel?.events));
  const extremes = $derived(gapExtremes(sel?.report?.points, spans));
  const trimStartS = $derived(sel?.report?.markers?.find((m) => m.kind === 'trim_start')?.t_s ?? null);

  const warnCount = $derived(
    sel ? sel.events.filter((e) => e.level === 'warn' || e.level === 'error').length : 0,
  );

  function closeSel() {
    sel = null;
    confirmDelete = false;
    pickRate = false;
  }
  function onKey(e) {
    if (e.key !== 'Escape') return;
    if (pickRate) pickRate = false;
    else if (confirmDelete) {
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
        rows.push([`refill_${i + 1}_fed_meanwhile_g`, fix(r.drawn_g, 1)]);
        rows.push([`refill_${i + 1}_poured_g`, fix(r.added_g, 1)]);
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
        placeholder="Search by run or person…"
        bind:value={q}
        aria-label="Search runs by name or person"
      />
      {#if pickedShown.length}
        <button class="btn-danger" onclick={() => { bulkErr = null; confirmBulk = true; }}>
          Delete selected ({pickedShown.length})
        </button>
      {/if}
    </div>
  </div>
  {#if err}<div class="err">{err}</div>{/if}

  <div class="tbl">
    <div class="row">
      <label class="pick" title={allPicked ? 'Unselect all' : 'Select all'}>
        <input type="checkbox" checked={allPicked} disabled={!pickable.length} onchange={toggleAll} aria-label="Select all runs" />
      </label>
      <div class="tr th">
        <span>Run</span><span>Name</span><span>Status</span><span>Curve</span><span>Ran</span><span>Started</span>
      </div>
    </div>
    {#each shown as r (r.id)}
      {@const rf = ranFor(r, now)}
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
          <span class="rid mono">#{r.id}</span>
          <span class="nm">{r.name}{#if r.responsible}<small class="who">{r.responsible}</small>{/if}</span>
          <span><span class="pill {r.status}">{r.status}</span></span>
          <span class="mono">{r.curve.params.kind}</span>
          <span class="mono ran">{rf.ran}{#if rf.of}<small>{rf.of}</small>{/if}</span>
          <span class="mono">{stampY(r.started_at)}</span>
        </button>
      </div>
    {:else}
      <p class="muted">{q.trim() ? `No run matches “${q.trim()}”.` : 'No runs yet.'}</p>
    {/each}
  </div>
  <!-- Out of the main action's place: deleting every run is rare and final. -->
  {#if runs.length}
    <div class="tbl-foot">
      <button
        class="link-danger"
        onclick={() => { clearErr = null; confirmClear = true; }}
        disabled={clearBlocked}
        title={clearBlocked ? 'Stop the run / pump first' : 'Delete every run'}
      >Reset history…</button>
    </div>
  {/if}
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
  {@const rf = ranFor(sel.run, now)}
  <button class="backdrop" aria-label="Close" onclick={closeSel}></button>
  <div class="layer">
    <div class="modal card detail" role="dialog" aria-modal="true" aria-labelledby="ft-run-detail">
      <!-- Sticky head: who and when, and what to do next, in reach however
           far the journal below is scrolled. -->
      <div class="card-head detail-head">
        <div class="who">
          <div class="idrow"><span class="pill {sel.run.status}">{sel.run.status}</span><span class="rid mono">#{sel.run.id}</span></div>
          <h2 id="ft-run-detail">{sel.run.name}</h2>
          <div class="when">
            <span class="nw">{whenLine(sel.run.started_at, sel.run.ended_at)}</span>
            <span class="sep">·</span><span class="nw">ran {rf.ran}{rf.of ? ` ${rf.of}` : ''}</span>
            {#if sel.run.responsible}<span class="sep">·</span><span class="nw">{sel.run.responsible}</span>{/if}
          </div>
          <!-- the curve said up front: its start and end were hard to find behind Settings -->
          <div class="chips" aria-label="Curve">
            {#each curveChips(sel.run) as c}
              <span class="chip">{#if c.value}<b class="mono">{c.value}</b> {c.unit}{:else}{c.text}{/if}</span>
            {/each}
          </div>
        </div>
        <div class="hd-actions">
          <div class="again">
            <button class="btn-primary" disabled={!replayable} onclick={runAgain}>Run again</button>
            {#if !replayable}<small>{notReplayable}</small>{/if}
          </div>
          <button class="x" aria-label="Close" title="Close (Esc)" onclick={closeSel}><Icon name="x" size={18} /></button>
        </div>
      </div>

      <!-- The answer first: did it feed what was asked? -->
      {#if verdictOf?.wrongSide}
        <p class="say"><b class="off">Balance on the wrong side.</b> The weighed figures are mirrored.</p>
      {:else if verdictOf}
        <p class="say">
          {#if verdictOf.onTarget}<b class="ok">On target.</b>{:else}<b class="off">Off target.</b>{/if}
          {#if verdictOf.onTarget && verdictOf.worstPct != null}
            Delivered stayed within {num(Math.max(verdictOf.worstPct, 0.1), 1)} % of the request after the first 15 min.
          {:else if verdictOf.onTarget}
            Delivered within ±{TOLERANCE * 100} % of the request.
          {:else}
            Delivered {num(Math.abs(verdictOf.gap), 1)} mL {verdictOf.gap < 0 ? 'short of' : 'over'} the request.
          {/if}
          Tolerance ±{TOLERANCE * 100} %.
        </p>
        <div class="figs">
          <div class="fig"><div class="v mono">{grouped(verdictOf.delivered)}<small>mL</small></div><div class="k">delivered, weighed</div></div>
          <div class="fig"><div class="v mono" class:off={!verdictOf.onTarget}>{num(verdictOf.pct, 1)}<small>%</small></div><div class="k">of {grouped(verdictOf.requested)} mL requested</div></div>
          <div class="fig"><div class="v mono">{nRefills}</div><div class="k">refill{nRefills === 1 ? '' : 's'}{nRefills ? (sel.run.auto_refill ? ', automatic' : ', by hand') : ''}</div></div>
          <div class="fig"><div class="v mono" class:off={alarms > 0}>{alarms}</div><div class="k">alarm{alarms === 1 ? '' : 's'}</div></div>
        </div>
      {:else}
        <p class="say">Not weighed: this run had no balance correction.</p>
      {/if}

      <div class="block">
        <div class="ctitle">
          <h3>Feed rate, {unitFor(sel.run.control_var)}</h3>
          <div class="legend">
            <span><i class="sw planned"></i>Requested</span>
            {#if chartWeighed.length}
              <span><i class="sw actual"></i>Weighed, {Math.round(flowWindow / 60)} min average</span>
            {:else}
              <span><i class="sw actual"></i>Sent to the pump</span>
            {/if}
            {#if spans.length}<span><i class="sw refilled"></i><i class="sw blind"></i>{blindLegend(spans.length, nRefills)}</span>{/if}
          </div>
        </div>
        <Chart
          planned={sel.planned}
          actual={chartActual}
          actualWidth={chartWeighed.length ? 1.75 : 2.5}
          nowS={null}
          durationS={reportSpanS}
          unit={unitFor(sel.run.control_var)}
          digits={digitsFor(sel.run.control_var)}
          bands={spans}
          blindBands={chartWeighed.length > 0}
          plannedWide
          endpoints={false}
          markers={marks.map((m) => ({ t: m.t, label: m.text }))}
        />
        {#if marks.length}
          <ol class="marks">
            {#each marks as m, i}<li><b>{i + 1}</b> {m.text}</li>{/each}
          </ol>
        {/if}
      </div>

      {#if verdictOf && !verdictOf.wrongSide && sel.report.points.length > 1}
        <div class="block">
          <div class="ctitle">
            <h3>Running total vs requested</h3>
            <div class="legend"><span>delivered − requested, mL</span></div>
          </div>
          <GapChart points={sel.report.points} {spans} spanS={reportSpanS} />
        </div>
      {/if}

      <!-- Settings open, in view; the rest, to look closer, folded. -->
      <div class="folds">
        <details class="fold" open>
          <summary>
            <Icon name="chevron-right" size={16} /><span class="lbl">Settings</span>
            <span class="peek">{[sel.cal ? 'tube' : null, sel.run.tubing_calibration_id != null ? 'calibration' : null, sel.run.gravimetric_trim ? 'balance' : null, sel.run.auto_refill ? 'refill' : null].filter(Boolean).join(', ') || 'details'}</span>
          </summary>
          <dl>
            {#if sel.cal}
              <dt>Tube</dt>
              <dd>{[sel.cal.brand, sel.cal.product_ref].filter(Boolean).join(' ') || 'Tube'}{sel.cal.inner_diameter_mm != null ? `, ${sel.cal.inner_diameter_mm} mm ID` : ''}{sel.cal.tubing_lot_id ? `, lot ${sel.cal.tubing_lot_id}` : ''}</dd>
            {/if}
            {#if sel.run.tubing_calibration_id != null}
              <dt>Calibration</dt>
              <dd>#{sel.run.tubing_calibration_id}{#if sel.cal}, in {unitFor(sel.cal.control_var)}{sel.run.control_var === 'ml_min' && sel.cal.control_var === 'rpm' ? ', driven in rpm' : ''}{/if}</dd>
            {/if}
            <dt>Balance correction</dt>
            <dd>{sel.run.gravimetric_trim ? `On, density ${num(sel.run.density_g_per_ml ?? 1, 2)} g/mL` : 'Off'}</dd>
            {#if sel.run.gravimetric_trim}
              <dt>Refill</dt>
              <dd>{sel.run.auto_refill ? `Automatic${sel.run.refill_draw_g ? `, after ${grouped(sel.run.refill_draw_g, 0)} g` : ''}` : 'By hand'}</dd>
            {/if}
            {#if sel.run.app_version}<dt>App</dt><dd class="mono">{sel.run.app_version}</dd>{/if}
          </dl>
        </details>

        <!-- the bottle's weights, folded: Settings first (2026-10-22) -->
        {#if bottle}
          <details class="fold">
            <summary>
              <Icon name="chevron-right" size={16} /><span class="lbl">Feed bottle</span>
              <span class="peek">{refills.length ? `${refills.length} refill${refills.length === 1 ? '' : 's'}, ${grouped(refills.reduce((a, r) => a + r.added_g, 0))} g poured` : 'no refill'}</span>
            </summary>
            <div class="tblwrap">
              <table class="bottle">
                <thead><tr><th>On the balance</th><th>Before</th><th>After</th><th>Poured</th><th>Fed during refill <span class="est-h">≈ est.</span></th></tr></thead>
                <tbody>
                  <tr><td>Start <span class="t">at {atRun(0)}</span></td><td class="dim">·</td><td>{grouped(bottle.start)} g</td><td class="dim">·</td><td class="dim">·</td></tr>
                  {#each refills as r, i}
                    <tr>
                      <td>Refill {i + 1} <span class="t">at {atRun(r.from_s)}{r.took_s != null ? `, ${mins(r.took_s)}` : ''}</span></td>
                      <td>{grouped(r.before_g)} g</td><td>{grouped(r.after_g)} g</td>
                      <td>{grouped(r.added_g)} g</td><td class="dim">≈ {grouped(r.drawn_g)} g</td>
                    </tr>
                  {/each}
                  <tr><td>End <span class="t">at {atRun(reportSpanS)}</span></td><td>{grouped(bottle.end)} g</td><td class="dim">·</td><td class="dim">·</td><td class="dim">·</td></tr>
                </tbody>
                {#if refills.length}
                  <tfoot><tr>
                    <td>{refills.length} refill{refills.length === 1 ? '' : 's'}</td><td></td><td></td>
                    <td>{grouped(refills.reduce((a, r) => a + r.added_g, 0))} g</td>
                    <td class="dim">≈ {grouped(refills.reduce((a, r) => a + r.drawn_g, 0))} g</td>
                  </tr></tfoot>
                {/if}
              </table>
            </div>
          </details>
        {/if}


        {#if sel.report}
          <details class="fold">
            <summary>
              <Icon name="chevron-right" size={16} /><span class="lbl">Regulation</span>
              <span class="peek">{[reg.ratio && `pump ratio ${reg.ratio}`, reg.correction && `correction ×${reg.correction}`].filter(Boolean).join(' · ')}</span>
            </summary>
            <dl>
              {#if trimStartS != null}
                <dt>Correction on</dt>
                <dd>at {atRun(trimStartS)}{reg.correction ? `, at ×${reg.correction}` : ''}</dd>
              {/if}
              {#if reg.ratio}<dt>Pump ratio</dt><dd class="mono">{reg.ratio}</dd>{/if}
              <dt>Largest gap</dt>
              <dd><span class="mono">+{num(extremes.ahead, 1)} mL</span> ahead, <span class="mono">−{num(Math.abs(extremes.behind), 1)} mL</span> behind</dd>
              <dt>Fit, R²</dt><dd class="mono">{sel.report.r_squared != null ? sel.report.r_squared.toFixed(4) : '·'}</dd>
            </dl>
          </details>
        {/if}

        {#if sel.events.length}
          <details class="fold">
            <summary>
              <Icon name="chevron-right" size={16} /><span class="lbl">Journal</span>
              <span class="peek">
                {journal.length} moment{journal.length === 1 ? '' : 's'} ·
                {#if warnCount}<b class="off">{warnCount} warning{warnCount === 1 ? '' : 's'}</b>{:else}no warning{/if}
              </span>
            </summary>
            {#if rawJournal}
              {#each sel.events as e (e.id)}
                <div class="act">
                  <time class="mono">{shortTime(e.wall_time)}</time>
                  <span class="pill {e.level}">{e.kind}</span>
                  <span class="body">{e.detail ?? ''}</span>
                </div>
              {/each}
            {:else}
              {#each journal as l}
                <div class="jr {l.tone}">
                  <span class="tm mono">{l.time}</span>
                  <span class="w"><b>{l.title}</b> <span>{l.detail}</span></span>
                </div>
              {/each}
            {/if}
            <button class="btn-ghost raw-btn" onclick={() => (rawJournal = !rawJournal)}>
              {rawJournal ? 'Show in words' : `Show all ${sel.events.length}${sel.events.length >= EVENTS_MAX ? '+' : ''} entries`}
            </button>
          </details>
        {/if}
      </div>

      <div class="modal-foot">
        <button class="link-danger del" disabled={sel.run.status === 'running'} onclick={() => { deleteErr = null; confirmDelete = true; }}>
          Delete run…
        </button>
        <button class="btn-ghost" disabled={!replayable} onclick={() => downloadSettings(settingsOf(sel.run))}
          title={replayable ? "Save this run's settings to a file, to import later in New run" : notReplayable}>
          Export settings
        </button>
        <button class="btn-ghost" disabled={exporting} onclick={exportCsv}
          title="Download the run's journal (one row per second)">
          {exporting ? 'Exporting…' : 'Export CSV'}
        </button>
      </div>
    </div>
  </div>

  <!-- Run again from which rate: in its own small dialog over the detail. -->
  {#if pickRate}
    {@const u = unitFor(sel.run.control_var)}
    {@const d = digitsFor(sel.run.control_var)}
    <button class="backdrop over" aria-label="Cancel" onclick={() => (pickRate = false)}></button>
    <div class="layer over">
      <div class="modal card confirm" role="dialog" aria-modal="true" aria-labelledby="ft-again">
        <div class="eyebrow">Run again</div>
        <h2 id="ft-again">Start from which rate?</h2>
        <p class="lede">The rate of “{sel.run.name}” changed during the run.</p>
        <fieldset class="rates" bind:this={ratesEl}>
          <legend class="sr-only">Rate</legend>
          {#each steps as s, i}
            <label class="rate" class:on={pickedRate === i}>
              <input type="radio" name="ft-again-rate" value={i} bind:group={pickedRate} />
              <span class="rv"><b class="mono">{num(s.value, d)}</b> {u}{#if s.atStart} <span class="dim">at the start</span>{:else if s.last} <span class="dim">the last one</span>{/if}</span>
              <span class="dim mono">{rateLabel(s)}</span>
            </label>
          {/each}
        </fieldset>
        <div class="modal-foot">
          <button class="btn-primary" onclick={runAgainAt}>Open New run at {num(steps[pickedRate].value, d)} {u}</button>
          <button class="btn-ghost" onclick={() => (pickRate = false)}>Cancel</button>
        </div>
      </div>
    </div>
  {/if}

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
    grid-template-columns: 0.45fr 1.5fr 0.8fr 0.8fr 0.9fr 1.2fr;
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
  .tr .rid { color: var(--muted); font-size: 12px; }
  .tr .ran small { display: block; font-size: 11.5px; color: var(--muted); }
  .tbl-foot { display: flex; justify-content: flex-end; margin-top: var(--s-3); }
  .link-danger {
    font: inherit; font-size: 13px; color: var(--danger-ink); background: none; border: none;
    padding: var(--s-1) var(--s-2); cursor: pointer; text-decoration: underline; text-underline-offset: 3px;
  }
  .link-danger:disabled { color: var(--muted); cursor: default; text-decoration: none; }
  .link-danger:focus-visible { outline: 2px solid var(--teal-600); outline-offset: 2px; border-radius: 4px; }
  .tr .who { display: block; font-size: 12px; font-weight: 400; color: var(--muted); }
  .tr span { font-size: 13px; }
  .muted { color: var(--muted); padding: var(--s-3) var(--s-2); }

  .hd-actions { display: flex; align-items: flex-start; gap: var(--s-2); flex: none; }
  /* No top padding on the detail: the sticky head is the top edge itself. */
  .modal.detail { padding-top: 0; max-width: 760px; }
  .detail-head { align-items: flex-start;
    position: sticky; top: 0; z-index: 2;
    margin: 0 calc(var(--s-7) * -1) var(--s-5);
    padding: var(--s-5) var(--s-7) var(--s-4);
    background: var(--surface);
    border-bottom: 1px solid var(--line);
  }
  .who { flex: 1; min-width: 0; }
  .idrow { display: flex; align-items: center; gap: var(--s-2); }
  .idrow .rid { font-size: 12px; color: var(--muted); }
  .detail-head h2 { margin: 4px 0 2px; font-size: 22px; text-wrap: balance; overflow-wrap: anywhere; }
  .when { display: flex; flex-wrap: wrap; column-gap: var(--s-2); font-size: 13px; color: var(--muted); }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; margin-top: var(--s-2); }
  .chip { padding: 3px 10px; border-radius: 999px; background: var(--surface-sunken); font-size: 13px; color: var(--ink); }
  .chip b { font-weight: 600; }
  .when .sep { opacity: 0.6; }
  .nw { white-space: nowrap; }
  .again { display: flex; flex-direction: column; align-items: flex-end; gap: 2px; }
  /* grey, not a paler teal: that read as "loading" */
  .again .btn-primary:disabled { background: var(--surface-sunken); border-color: var(--line); color: var(--muted); }
  .again small { font-size: 11px; color: var(--muted); max-width: 160px; text-align: right; line-height: 1.35; }
  .x {
    width: 36px; height: 36px; display: grid; place-items: center; flex: none;
    border: none; background: none; color: var(--muted); border-radius: var(--radius-ctl); cursor: pointer;
  }
  .x:hover { background: var(--surface-sunken); color: var(--ink); }
  .x:focus-visible { outline: 2px solid var(--teal-600); outline-offset: 2px; }

  /* the verdict */
  .say { margin: 0 0 var(--s-4); font-size: 15px; max-width: 62ch; }
  .say .ok { color: var(--green-ink); }
  .say .off, .fig .off { color: var(--danger-ink); }
  .figs { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); border: 1px solid var(--line); border-radius: var(--radius); }
  .fig { padding: var(--s-3) var(--s-4); min-width: 0; }
  .fig + .fig { border-left: 1px solid var(--line); }
  .fig .v { font-size: 22px; font-weight: 600; line-height: 1.2; letter-spacing: -0.02em; font-variant-numeric: tabular-nums; }
  .fig .v small { font-size: 13px; font-weight: 500; color: var(--muted); margin-left: 3px; letter-spacing: 0; }
  .fig .k { font-size: 12px; color: var(--muted); margin-top: 2px; line-height: 1.4; }

  .block { margin-top: var(--s-5); }
  .block h3 { margin: 0 0 var(--s-2); font-size: 14px; font-weight: 600; }
  .ctitle { display: flex; justify-content: space-between; align-items: baseline; gap: var(--s-3); flex-wrap: wrap; margin-bottom: var(--s-2); }
  .ctitle h3 { margin: 0; }

  /* the bottle */
  .tblwrap { overflow-x: auto; }
  .bottle { width: 100%; border-collapse: collapse; font-size: 13px; }
  .bottle th {
    text-align: right; font-weight: 500; font-size: 11px; letter-spacing: 0.06em; text-transform: uppercase;
    color: var(--muted); padding: 0 var(--s-3) var(--s-2); white-space: nowrap;
  }
  .bottle td {
    text-align: right; padding: 7px var(--s-3); border-top: 1px solid var(--line-soft);
    font-family: var(--mono); font-variant-numeric: tabular-nums; white-space: nowrap;
  }
  .bottle th:first-child, .bottle td:first-child { text-align: left; padding-left: 0; font-family: inherit; }
  .bottle th:last-child, .bottle td:last-child { padding-right: 0; }
  .bottle .t { font-family: var(--mono); font-size: 12px; color: var(--muted); margin-left: 6px; }
  .bottle .dim { color: var(--muted); }
  .bottle tfoot td { border-top: 1px solid var(--line); font-weight: 600; }
  .est-h { text-transform: none; letter-spacing: 0; font-weight: 400; }

  /* folded sections */
  .folds { margin-top: var(--s-5); }
  .lbl { font-weight: 600; }
  dl { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 6px var(--s-5); margin: 0 0 var(--s-4) 24px; font-size: 13px; }
  dt { color: var(--muted); }
  dd { margin: 0; }
  .jr { display: grid; grid-template-columns: 58px minmax(0, 1fr); gap: var(--s-3); padding: 6px 0 6px 24px; font-size: 13px; align-items: baseline; }
  .jr + .jr { border-top: 1px solid var(--line-soft); }
  .jr .tm { font-size: 12px; color: var(--muted); }
  .jr .w b { font-weight: 600; }
  .jr .w span { color: var(--muted); }
  .jr.refill .w b { color: var(--fill-ink); }
  .jr.bad .w b { color: var(--danger-ink); }
  .raw-btn { margin: var(--s-2) 0 var(--s-4) 24px; padding: 4px 10px; font-size: 12px; }

  .fold { border-top: 1px solid var(--line); }
  .fold:last-child { border-bottom: 1px solid var(--line); }
  .fold summary {
    display: flex; align-items: center; gap: var(--s-2); padding: var(--s-3) 0;
    cursor: pointer; list-style: none; border-radius: var(--radius-ctl);
  }
  .fold summary::-webkit-details-marker { display: none; }
  .fold summary :global(.icon) { color: var(--muted); transition: transform 0.15s ease; }
  .fold[open] summary :global(.icon) { transform: rotate(90deg); }
  .fold summary:focus-visible { outline: 2px solid color-mix(in srgb, var(--teal-700) 45%, transparent); outline-offset: 2px; }
  .peek { margin-left: auto; font-size: 12px; color: var(--muted); text-align: right; }
  .peek .off { color: var(--danger-ink); font-weight: 600; }
  @media (prefers-reduced-motion: reduce) { .fold summary :global(.icon) { transition: none; } }

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
  .marks { list-style: none; margin: var(--s-2) 0 0; padding: 0; display: flex; flex-wrap: wrap; gap: var(--s-1) var(--s-5); font-size: 12.5px; color: var(--muted); }
  .marks b { color: var(--ink); font-weight: 600; font-family: var(--mono); }
  .rates { margin: var(--s-4) 0 0; padding: 0; border: 1px solid var(--line); border-radius: var(--radius-ctl); }
  .rate { display: flex; align-items: center; gap: var(--s-3); padding: var(--s-3) var(--s-3); cursor: pointer; font-size: 14px; }
  .rate + .rate { border-top: 1px solid var(--line); }
  .rate.on { background: color-mix(in srgb, var(--teal-700) 7%, var(--surface)); }
  .rate input { margin: 0; accent-color: var(--teal-700); }
  .rate .rv { flex: 1; }
  .rate b { font-weight: 600; }
  .rate .dim { color: var(--muted); font-size: 13px; }
  .legend { display: flex; flex-wrap: wrap; gap: var(--s-1) var(--s-4); font-size: 12px; color: var(--muted); }
  .legend span { display: inline-flex; align-items: center; gap: 6px; white-space: nowrap; }
  .sw { display: inline-block; width: 16px; height: 3px; border-radius: 2px; }
  .sw.planned { background: var(--teal-400); height: 6px; opacity: 0.45; }
  .sw.actual { background: var(--green-500); }
  .sw.refilled { width: 12px; height: 12px; border-radius: 3px; background: color-mix(in srgb, var(--fill) 16%, transparent); }
  .sw.blind { background: repeating-linear-gradient(90deg, var(--muted) 0 4px, transparent 4px 7px); }

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

  .act {
    display: grid; grid-template-columns: 60px 120px 1fr; gap: var(--s-3);
    align-items: baseline; padding: var(--s-2) 0 var(--s-2) 24px; font-size: 12px;
    color: var(--muted);
  }
  .act + .act { border-top: 1px solid var(--line-soft); }
  .act time { color: var(--muted); font-size: 12px; }
  .act .body { color: var(--muted); }

  @media (max-width: 720px) {
    .detail-head { flex-wrap: wrap; }
    .hd-actions { width: 100%; justify-content: space-between; }
    .again { align-items: flex-start; }
    /* grey, not a paler teal: that read as "loading" */
  .again .btn-primary:disabled { background: var(--surface-sunken); border-color: var(--line); color: var(--muted); }
  .again small { text-align: left; }
    .figs { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .fig:nth-child(3) { border-left: none; }
    .fig:nth-child(n + 3) { border-top: 1px solid var(--line); }
    .tr { grid-template-columns: 0.5fr 1.4fr 0.9fr 0.9fr; }
    .tr > span:nth-child(4), .tr > span:nth-child(6) { display: none; }
  }
</style>
