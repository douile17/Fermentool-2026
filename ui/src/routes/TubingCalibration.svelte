<script>
  // Tubing calibration: three timed bursts at one setpoint, each weighed by
  // hand, recorded as one calibration per tubing lot + size. The balance is
  // never read here on purpose: the bench where a tube is calibrated need not
  // be wired to this Fermentool. Each burst is an ordinary run
  // (kind = calibration); its real duration comes from the run's own clock,
  // the target duration below only sets when the pump stops by itself.
  import { app } from '../lib/state.svelte.js';
  import { get, post, del } from '../lib/api.js';
  import { num, clock, stamp, unitFor, digitsFor, RPM_LIMITS, FLOW_LIMITS } from '../lib/fmt.js';
  import { canStartRun } from '../lib/link.js';
  import ErrorText from '../components/ErrorText.svelte';

  // Mirrors trim::CALIBRATION_CV_WARN_PCT and the trim bounds: warnings only.
  const CV_WARN_PCT = 5;
  const C0_MIN = 0.8;
  const C0_MAX = 1.25;

  const blank = () => ({
    tubing_lot_id: '',
    tubing_size: '',
    inner_diameter_mm: null,
    outer_diameter_mm: null,
    control_var: 'ml_min',
    direction: 'cw',
    setpoint: 10,
    density_g_per_ml: 1.0,
    target_min: 5,
    operator: '',
    note: '',
    // [{ run_id, weight_g }], at most three
    bursts: [],
  });

  let d = $state(blank());
  let loaded = $state(false);
  let err = $state(null);
  let working = $state(false);
  let saved = $state(null); // the stored row, authoritative
  let durations = $state({}); // run_id -> minutes, from the run rows
  let history = $state([]);

  let pumpAddr = $state(1);
  $effect(() => {
    get('/api/config')
      .then((c) => (pumpAddr = c.pump.address))
      .catch(() => {});
  });

  // Resume an in-progress session (survives a refresh or a daemon restart).
  $effect(() => {
    get('/api/calibrations/draft')
      .then((draft) => {
        if (draft && typeof draft === 'object') Object.assign(d, blank(), draft);
      })
      .catch(() => {})
      .finally(() => (loaded = true));
    loadHistory();
  });

  function loadHistory() {
    get('/api/calibrations')
      .then((rows) => (history = rows ?? []))
      .catch(() => {});
  }

  function persist() {
    return post('/api/calibrations/draft', $state.snapshot(d)).catch(() => {});
  }

  const unit = $derived(unitFor(d.control_var));
  const locked = $derived(d.bursts.length > 0);
  const active = $derived(app.status?.active ?? null);
  const current = $derived(d.bursts.at(-1) ?? null);
  const burstRunning = $derived(!!current && active?.run_id === current.run_id);
  const otherRunActive = $derived(!!active && !burstRunning);
  const needsWeight = $derived(!!current && !burstRunning && current.weight_g == null);
  const complete = $derived(
    d.bursts.length === 3 && d.bursts.every((b) => b.weight_g != null && b.weight_g > 0),
  );

  const formOk = $derived(
    d.tubing_lot_id.trim() !== '' &&
      d.tubing_size.trim() !== '' &&
      Number(d.inner_diameter_mm) > 0 &&
      Number(d.outer_diameter_mm) > Number(d.inner_diameter_mm) &&
      Number(d.setpoint) > 0 &&
      Number(d.density_g_per_ml) > 0 &&
      Number(d.target_min) > 0,
  );
  const canStartBurst = $derived(
    loaded &&
      formOk &&
      !working &&
      !active &&
      canStartRun(app.status) &&
      d.bursts.length < 3 &&
      (current == null || current.weight_g != null),
  );

  // Countdown for the burst in progress, from the run's own start and length.
  let now = $state(Date.now());
  $effect(() => {
    if (!burstRunning) return;
    const t = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(t);
  });
  const remainingS = $derived.by(() => {
    if (!burstRunning || !active) return null;
    const end = Date.parse(active.started_at) + active.duration_s * 1000;
    return Math.max(0, (end - now) / 1000);
  });

  async function startBurst() {
    err = null;
    working = true;
    const n = d.bursts.length + 1;
    const setpoint = Number(d.setpoint);
    const lim = d.control_var === 'ml_min' ? FLOW_LIMITS : RPM_LIMITS;
    try {
      const r = await post('/api/runs', {
        name: `calibration ${d.tubing_lot_id.trim()} ${d.tubing_size.trim()} ${n}/3`,
        control_var: d.control_var,
        direction: d.direction,
        pump_addr: pumpAddr,
        curve: {
          mode: 'endpoints',
          start: setpoint,
          end: setpoint,
          duration: Math.round(Number(d.target_min) * 60),
          clamp_min: lim.min,
          clamp_max: lim.max,
          params: { kind: 'constant' },
        },
        gravimetric_trim: false,
        kind: 'calibration',
      });
      d.bursts.push({ run_id: r.run_id, weight_g: null });
      await persist();
    } catch (e) {
      err = { message: e.message, hint: e.hint ?? null };
    } finally {
      working = false;
    }
  }

  async function stopBurst() {
    err = null;
    working = true;
    try {
      await post(`/api/runs/${current.run_id}/stop`);
    } catch (e) {
      err = { message: e.message, hint: null };
    } finally {
      working = false;
    }
  }

  let weightInput = $state('');
  async function saveWeight() {
    const w = Number(weightInput);
    if (!(w > 0)) return;
    current.weight_g = w;
    weightInput = '';
    await persist();
  }

  async function redoLast() {
    d.bursts.pop();
    await persist();
  }

  // Two-step, in-page: a native confirm() dialog is not something the
  // desktop webview can be relied on to show.
  let confirmDiscard = $state(false);
  async function discard() {
    confirmDiscard = false;
    await del('/api/calibrations/draft').catch(() => {});
    Object.assign(d, blank());
    saved = null;
    err = null;
  }

  // Real burst durations, for the preview only.
  $effect(() => {
    for (const b of d.bursts) {
      if (b.weight_g == null || durations[b.run_id] != null) continue;
      get(`/api/runs/${b.run_id}`)
        .then((run) => {
          if (run?.ended_at) {
            durations[b.run_id] =
              (Date.parse(run.ended_at) - Date.parse(run.started_at)) / 60000;
          }
        })
        .catch(() => {});
    }
  });

  // Same math as trim::compute_calibration, for immediate feedback only. The
  // numbers that get stored come back from POST /api/calibrations.
  function compute(setpoint, density, samples) {
    const measured = samples.map(([min, g]) => g / (density * min));
    const mean = measured.reduce((a, b) => a + b, 0) / measured.length;
    const variance = measured.reduce((a, m) => a + (m - mean) ** 2, 0) / measured.length;
    return {
      measured_ml_min: measured,
      mean_measured_ml_min: mean,
      cv_pct: mean > 0 ? (100 * Math.sqrt(variance)) / mean : 0,
      c0: mean > 0 ? setpoint / mean : 1,
    };
  }

  const preview = $derived.by(() => {
    if (!complete) return null;
    const samples = d.bursts.map((b) => [durations[b.run_id], b.weight_g]);
    if (samples.some(([min]) => !(min > 0))) return null;
    return compute(Number(d.setpoint), Number(d.density_g_per_ml), samples);
  });
  const shown = $derived(saved ?? preview);

  async function record() {
    err = null;
    working = true;
    try {
      saved = await post('/api/calibrations', {
        tubing_lot_id: d.tubing_lot_id.trim(),
        tubing_size: d.tubing_size.trim(),
        inner_diameter_mm: Number(d.inner_diameter_mm),
        outer_diameter_mm: Number(d.outer_diameter_mm),
        control_var: d.control_var,
        setpoint: Number(d.setpoint),
        density_g_per_ml: Number(d.density_g_per_ml),
        run_ids: d.bursts.map((b) => b.run_id),
        weights_g: d.bursts.map((b) => b.weight_g),
        operator: d.operator.trim() || null,
        note: d.note.trim() || null,
      });
      // The server drops the draft once the record is stored.
      Object.assign(d, blank());
      loadHistory();
    } catch (e) {
      err = { message: e.message, hint: e.hint ?? null };
    } finally {
      working = false;
    }
  }

  function exportCsv() {
    const cols = [
      'id', 'created_at', 'tubing_lot_id', 'tubing_size', 'inner_diameter_mm',
      'outer_diameter_mm', 'control_var', 'setpoint',
      'density_g_per_ml', 'run_1_id', 'run_2_id', 'run_3_id', 'weight_1_g', 'weight_2_g',
      'weight_3_g', 'measured_1_ml_min', 'measured_2_ml_min', 'measured_3_ml_min',
      'mean_measured_ml_min', 'cv_pct', 'c0', 'operator', 'note',
    ];
    const cell = (v) => {
      const s = v == null ? '' : String(v);
      return /[",\n]/.test(s) ? `"${s.replaceAll('"', '""')}"` : s;
    };
    const lines = history.map((c) =>
      [
        c.id, c.created_at, c.tubing_lot_id, c.tubing_size, c.inner_diameter_mm,
        c.outer_diameter_mm, c.control_var, c.setpoint,
        c.density_g_per_ml, ...c.run_ids, ...c.weights_g, ...c.measured_ml_min,
        c.mean_measured_ml_min, c.cv_pct, c.c0, c.operator, c.note,
      ].map(cell).join(','),
    );
    const blob = new Blob([[cols.join(','), ...lines].join('\n') + '\n'], { type: 'text/csv' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = 'tubing-calibrations.csv';
    a.click();
    URL.revokeObjectURL(a.href);
  }

  const cvBad = (cv) => cv > CV_WARN_PCT;
  const c0Bad = (cv, c0) => cv === 'ml_min' && (c0 < C0_MIN || c0 > C0_MAX);
</script>

<section class="card">
  <div class="card-head">
    <div>
      <h2>Tubing calibration</h2>
      <p class="sub">
        Three bursts at one setpoint, each weighed by hand. Required before a gravimetric trim in
        rpm, recommended in ml/min. Calibrate a tube before it is autoclaved and installed.
      </p>
    </div>
  </div>

  {#if !loaded}
    <p class="muted">Loading…</p>
  {:else}
    <div class="group">
      <div class="eyebrow">Tube and setpoint</div>
      <div class="grid">
        <label class="field"><span>Tubing lot</span>
          <input type="text" bind:value={d.tubing_lot_id} disabled={locked} onchange={persist} placeholder="e.g. LOT-2409" />
        </label>
        <label class="field"><span>Tubing size</span>
          <input type="text" bind:value={d.tubing_size} disabled={locked} onchange={persist} placeholder="e.g. #16" />
        </label>
        <label class="field"><span>Inner Ø (mm)</span>
          <input type="number" step="0.1" min="0" bind:value={d.inner_diameter_mm} disabled={locked} onchange={persist} />
        </label>
        <label class="field"><span>Outer Ø (mm)</span>
          <input type="number" step="0.1" min="0" bind:value={d.outer_diameter_mm} disabled={locked} onchange={persist} />
        </label>
        <div class="field"><span>Control</span>
          <div class="seg">
            <button disabled={locked} class:on={d.control_var === 'ml_min'} onclick={() => { d.control_var = 'ml_min'; persist(); }}>ml/min</button>
            <button disabled={locked} class:on={d.control_var === 'rpm'} onclick={() => { d.control_var = 'rpm'; persist(); }}>rpm</button>
          </div>
        </div>
        <div class="field"><span>Direction</span>
          <div class="seg">
            <button disabled={locked} class:on={d.direction === 'cw'} onclick={() => { d.direction = 'cw'; persist(); }}>clockwise</button>
            <button disabled={locked} class:on={d.direction === 'ccw'} onclick={() => { d.direction = 'ccw'; persist(); }}>counter</button>
          </div>
        </div>
        <label class="field"><span>Setpoint ({unit})</span>
          <input type="number" step="0.1" min="0" bind:value={d.setpoint} disabled={locked} onchange={persist} />
        </label>
        <label class="field"><span>Feed density (g/mL)</span>
          <input type="number" step="0.01" min="0" bind:value={d.density_g_per_ml} disabled={locked} onchange={persist} />
        </label>
        <label class="field"><span>Burst length (min, timer only)</span>
          <input type="number" step="0.5" min="0" bind:value={d.target_min} onchange={persist} />
        </label>
        <label class="field"><span>Operator (optional)</span>
          <input type="text" bind:value={d.operator} onchange={persist} />
        </label>
      </div>
    </div>

    <div class="group">
      <div class="eyebrow">Bursts</div>
      <ol class="bursts">
        {#each [0, 1, 2] as i}
          {@const b = d.bursts[i]}
          <li class:done={b?.weight_g != null}>
            <span class="n mono">{i + 1}/3</span>
            {#if !b}
              <span class="muted">not started</span>
            {:else if active?.run_id === b.run_id}
              <span>pumping, run #{b.run_id}{remainingS != null ? `, ${clock(remainingS)} left` : ''}</span>
            {:else if b.weight_g == null}
              <span>stopped, run #{b.run_id}: weigh the collected feed</span>
            {:else}
              <span class="mono">{num(b.weight_g, 2)} g{durations[b.run_id] ? ` in ${num(durations[b.run_id], 2)} min` : ''}</span>
            {/if}
          </li>
        {/each}
      </ol>

      {#if Number(d.inner_diameter_mm) > 0 && Number(d.outer_diameter_mm) > 0 && Number(d.outer_diameter_mm) <= Number(d.inner_diameter_mm)}
        <p class="warn">The outer Ø must be larger than the inner Ø.</p>
      {/if}
      {#if otherRunActive}
        <p class="warn">Another run is active. Stop it before starting a burst.</p>
      {/if}

      <div class="actions">
        {#if burstRunning}
          <button class="btn-primary" disabled={working} onclick={stopBurst}>Stop and weigh</button>
        {:else if needsWeight}
          <label class="field weigh"><span>Collected weight (g)</span>
            <input type="number" step="0.01" min="0" bind:value={weightInput} onkeydown={(e) => e.key === 'Enter' && saveWeight()} />
          </label>
          <button class="btn-primary" disabled={!(Number(weightInput) > 0)} onclick={saveWeight}>Save weight</button>
        {:else if d.bursts.length < 3}
          <button class="btn-primary" disabled={!canStartBurst} onclick={startBurst}>
            {working ? 'Starting…' : `Start burst ${d.bursts.length + 1}/3`}
          </button>
        {/if}
        {#if current && !burstRunning}
          <button class="btn-ghost" onclick={redoLast}>Redo burst {d.bursts.length}</button>
        {/if}
        {#if confirmDiscard}
          <span class="confirm">Discard this session? The bursts stay in the run history.</span>
          <button class="btn-danger" onclick={discard}>Discard</button>
          <button class="btn-ghost" onclick={() => (confirmDiscard = false)}>Keep</button>
        {:else if locked || d.tubing_lot_id}
          <button class="btn-danger" onclick={() => (confirmDiscard = true)}>Discard session</button>
        {/if}
      </div>
    </div>

    {#if shown}
      <div class="group">
        <div class="eyebrow">{saved ? 'Recorded calibration' : 'Preview (recorded values come from the daemon)'}</div>
        <div class="result mono">
          <div>flows {shown.measured_ml_min.map((m) => num(m, 3)).join(' · ')} ml/min</div>
          <div>mean <b>{num(shown.mean_measured_ml_min, 3)} ml/min</b></div>
          <div class:bad={cvBad(shown.cv_pct)}>CV {num(shown.cv_pct, 2)} %</div>
          {#if (saved?.control_var ?? d.control_var) === 'ml_min'}
            <div class:bad={c0Bad('ml_min', shown.c0)}>c₀ {num(shown.c0, 4)}</div>
          {:else}
            <div>{num(shown.mean_measured_ml_min / (saved?.setpoint ?? Number(d.setpoint)), 4)} ml/min per rpm</div>
          {/if}
        </div>
        {#if cvBad(shown.cv_pct)}
          <p class="warn">The three bursts disagree by more than {CV_WARN_PCT} %. Check for air bubbles, a leak or a missed drop, and consider redoing them.</p>
        {/if}
        {#if c0Bad(saved?.control_var ?? d.control_var, shown.c0)}
          <p class="warn">c₀ is outside {C0_MIN} to {C0_MAX}: the pump delivers far from its setpoint. The trim starts clamped to that range.</p>
        {/if}
        {#if !saved}
          <label class="field"><span>Note (optional)</span>
            <input type="text" bind:value={d.note} onchange={persist} />
          </label>
          <div class="actions">
            <button class="btn-primary" disabled={working} onclick={record}>Record calibration</button>
          </div>
        {:else}
          <p class="muted">Recorded as calibration #{saved.id}. Pick it in New run when you enable the gravimetric trim.</p>
        {/if}
      </div>
    {/if}

    {#if err}
      <div style="margin-top:16px"><ErrorText message={err.message} hint={err.hint} /></div>
    {/if}

    <div class="group">
      <div class="eyebrow row">
        <span>Recorded calibrations</span>
        {#if history.length}<button class="btn-ghost" onclick={exportCsv}>Export CSV</button>{/if}
      </div>
      {#if history.length}
        <table class="hist mono">
          <thead>
            <tr><th>#</th><th>date</th><th>lot</th><th>size</th><th>Ø int / ext (mm)</th><th>setpoint</th><th>mean ml/min</th><th>CV %</th><th>c₀</th></tr>
          </thead>
          <tbody>
            {#each history as c (c.id)}
              <tr>
                <td>{c.id}</td>
                <td>{stamp(c.created_at)}</td>
                <td>{c.tubing_lot_id}</td>
                <td>{c.tubing_size}</td>
                <td>{num(c.inner_diameter_mm, 1)} / {num(c.outer_diameter_mm, 1)}</td>
                <td>{num(c.setpoint, digitsFor(c.control_var))} {unitFor(c.control_var)}</td>
                <td>{num(c.mean_measured_ml_min, 3)}</td>
                <td class:bad={cvBad(c.cv_pct)}>{num(c.cv_pct, 2)}</td>
                <td class:bad={c0Bad(c.control_var, c.c0)}>{c.control_var === 'ml_min' ? num(c.c0, 4) : '·'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {:else}
        <p class="muted">None yet.</p>
      {/if}
    </div>
  {/if}
</section>

<style>
  .sub { margin: var(--s-1) 0 0; font-size: 13px; color: var(--muted); max-width: 60ch; }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: var(--s-4);
  }
  .group + .group {
    margin-top: var(--s-5);
    padding-top: var(--s-5);
    border-top: 1px solid color-mix(in srgb, var(--muted) 32%, transparent);
  }
  .group .eyebrow { margin-bottom: var(--s-3); color: var(--ink); }
  .eyebrow.row { display: flex; align-items: center; justify-content: space-between; }
  .muted { color: var(--muted); }
  .bursts { list-style: none; margin: 0 0 var(--s-4); padding: 0; display: grid; gap: var(--s-2); }
  .bursts li { display: flex; gap: var(--s-3); align-items: baseline; font-size: 13px; }
  .bursts .n { color: var(--muted); min-width: 3ch; }
  .bursts li.done .n { color: var(--green-600); }
  .actions { display: flex; gap: var(--s-3); align-items: flex-end; flex-wrap: wrap; margin-top: var(--s-3); }
  .weigh { width: 180px; }
  .confirm { font-size: 13px; align-self: center; }
  .warn { font-size: 13px; color: var(--danger); margin: var(--s-3) 0 0; }
  .result {
    display: flex;
    flex-wrap: wrap;
    gap: var(--s-2) var(--s-5);
    font-size: 13px;
    padding: var(--s-3);
    background: var(--surface-sunken);
    border-radius: var(--radius-ctl);
  }
  .bad { color: var(--danger); }
  .hist { width: 100%; border-collapse: collapse; font-size: 12px; }
  .hist th { text-align: left; color: var(--muted); font-weight: 500; padding: var(--s-1) var(--s-2); }
  .hist td { padding: var(--s-1) var(--s-2); border-top: 1px solid var(--line); }
  @media (max-width: 720px) {
    .hist { display: block; overflow-x: auto; }
  }
</style>
