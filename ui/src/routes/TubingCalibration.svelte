<script>
  // Tubing calibration: three timed bursts at one setpoint, each weighed,
  // recorded as one calibration per tubing lot and diameters. A balance
  // connected to this Fermentool, under the feed bottle, is read before and
  // after each burst (by itself in automatic mode); without one, or on
  // another bench, the weight is typed in.
  // Each burst is an ordinary run
  // (kind = calibration); its real duration comes from the run's own clock,
  // the target duration below only sets when the pump stops by itself.
  import { app } from '../lib/state.svelte.js';
  import { get, post, del } from '../lib/api.js';
  import { num, dur, stamp, unitFor, digitsFor, RPM_LIMITS, FLOW_LIMITS } from '../lib/fmt.js';
  import { canStartRun } from '../lib/link.js';
  import ErrorText from '../components/ErrorText.svelte';

  // Mirrors trim::CALIBRATION_CV_WARN_PCT and the trim bounds (Settings,
  // correction limit): warnings only.
  const CV_WARN_PCT = 5;
  let trimLimit = $state(25);
  const C0_MAX = $derived(Number((1 + trimLimit / 100).toFixed(2)));
  const C0_MIN = $derived(Number((1 / (1 + trimLimit / 100)).toFixed(2)));

  const blank = () => ({
    tubing_lot_id: '',
    internal_ref: '',
    inner_diameter_mm: null,
    outer_diameter_mm: null,
    control_var: 'ml_min',
    direction: 'cw',
    setpoint: 10,
    density_g_per_ml: 1.0,
    target_min: 5,
    operator: '',
    note: '',
    // run the three bursts and their weighing without a click (balance needed)
    auto: false,
    // the page (tab, window) driving the automatic mode: only that one acts
    auto_owner: null,
    // [{ run_id, weight_g, start_g }], at most three; start_g is the balance
    // reading just before the burst (null without a balance)
    bursts: [],
  });

  let d = $state(blank());
  // This page's identity for the automatic mode: with the page open in two
  // places (the desktop window and a browser tab), both used to start bursts
  // and save their own copy of the draft over the other's.
  const PAGE = crypto.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const autoHere = $derived(d.auto && d.auto_owner === PAGE);
  const autoElsewhere = $derived(d.auto && d.auto_owner !== PAGE);
  let loaded = $state(false);
  let err = $state(null);
  let working = $state(false);
  let saved = $state(null); // the stored row, authoritative
  let durations = $state({}); // run_id -> minutes, from the run rows
  let history = $state([]);

  let pumpAddr = $state(1);
  $effect(() => {
    get('/api/config')
      .then((c) => {
        pumpAddr = c.pump.address;
        trimLimit = c.scale?.trim_limit_pct ?? 25;
      })
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

  // Archived tubes stay on record (past runs point at them) but leave New run.
  let showArchived = $state(false);
  let archivedCount = $derived(history.filter((c) => c.archived_at).length);
  let listed = $derived(showArchived ? history : history.filter((c) => !c.archived_at));
  let archiving = $state(null); // id being archived/restored

  async function setArchived(c, archived) {
    err = null;
    archiving = c.id;
    try {
      const row = await post(`/api/calibrations/${c.id}/${archived ? 'archive' : 'restore'}`);
      history = history.map((h) => (h.id === row.id ? row : h));
    } catch (e) {
      err = { message: e.message, hint: null };
    } finally {
      archiving = null;
    }
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

  // The connected balance, read once a second while no run is pumping. Its
  // weight change over a burst is the collected mass, whichever side it is
  // on (under the collecting beaker, or under the feed bottle).
  const scaleW = $derived(app.status?.scale_weight_g ?? null);
  const scaleStable = $derived(app.status?.scale_stable === true);
  const collectedG = $derived(
    current?.start_g != null && scaleW != null ? Math.abs(scaleW - current.start_g) : null,
  );
  const complete = $derived(
    d.bursts.length === 3 && d.bursts.every((b) => b.weight_g != null && b.weight_g > 0),
  );

  const formOk = $derived(
    d.tubing_lot_id.trim() !== '' &&
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
    const startG = scaleW; // before the pump moves
    try {
      const r = await post('/api/runs', {
        name: `calibration ${d.tubing_lot_id.trim()} ${n}/3`,
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
      d.bursts.push({ run_id: r.run_id, weight_g: null, start_g: startG });
      startedHere = r.run_id;
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

  // A burst left pumping by an older session (e.g. one discarded before
  // discard stopped the pump). Only calibration runs: a dosing run is stopped
  // from its own panel.
  async function stopOther() {
    err = null;
    working = true;
    try {
      await post(`/api/runs/${active.run_id}/stop`);
    } catch (e) {
      err = { message: e.message, hint: null };
    } finally {
      working = false;
    }
  }

  async function saveBalanceReading() {
    if (!(collectedG > 0)) return;
    current.weight_g = Number(collectedG.toFixed(2));
    weightInput = '';
    await persist();
  }

  // Full auto. With the balance under the feed bottle nothing needs a hand
  // between bursts: each one starts once the balance has been still for
  // STEADY_S, stops by itself at its duration, and is weighed once the
  // balance is still again. Driven from this page: closing it pauses the
  // sequence where it stands, the draft (and `auto`) resumes it.
  const STEADY_S = 5;
  // Right after a stop the status may still carry the reading from before
  // the burst (the balance is not read while a run pumps).
  const AFTER_STOP_S = 3;
  // Less than this gone from the bottle after a whole burst: the balance is
  // not under it, or the line is not drawing.
  const MIN_COLLECTED_G = 1;
  let stableSince = $state(null);
  let stoppedAt = $state(null);
  let wasRunning = false;
  // A burst this page just started reads as stopped until the status shows
  // it running (the start reply can beat the status push): not weighable yet.
  let startedHere = null;
  const seenRunning = new Set();
  let tick = $state(Date.now());
  $effect(() => {
    if (!d.auto) return;
    const t = setInterval(() => (tick = Date.now()), 500);
    return () => clearInterval(t);
  });
  $effect(() => {
    if (scaleStable && scaleW != null) {
      if (stableSince == null) stableSince = Date.now();
    } else {
      stableSince = null;
    }
  });
  $effect(() => {
    const r = burstRunning;
    if (r && current) seenRunning.add(current.run_id);
    if (wasRunning && !r) stoppedAt = Date.now();
    wasRunning = r;
  });
  const steadyS = $derived(
    stableSince == null ? 0 : (tick - Math.max(stableSince, (stoppedAt ?? 0) + AFTER_STOP_S * 1000)) / 1000,
  );
  const canAuto = $derived(scaleW != null && !complete && !otherRunActive && (current == null || current.start_g != null || current.weight_g != null));
  const autoStep = $derived(
    burstRunning
      ? `burst ${d.bursts.length}/3 pumping`
      : needsWeight
        ? 'waiting for the balance to settle, then weighing'
        : `burst ${d.bursts.length + 1}/3 starts once the balance is still`,
  );

  async function setAuto(on) {
    d.auto = on;
    d.auto_owner = on ? PAGE : null;
    err = null;
    await persist();
  }

  $effect(() => {
    if (!autoHere || working || burstRunning) return;
    if (complete) {
      setAuto(false);
      return;
    }
    if (steadyS < STEADY_S) return;
    if (needsWeight && current.run_id === startedHere && !seenRunning.has(current.run_id)) return;
    if (needsWeight) {
      if (collectedG != null && collectedG >= MIN_COLLECTED_G) {
        saveBalanceReading();
      } else {
        err = {
          message: `The balance moved by ${num(collectedG ?? 0, 2)} g over the burst: automatic mode stopped.`,
          hint: 'Is the feed bottle on the balance, and the line drawing from it? Weigh this burst by hand, or redo it.',
        };
        d.auto = false;
        d.auto_owner = null;
        persist();
      }
    } else if (canStartBurst) {
      startBurst();
    }
  });

  let weightInput = $state('');
  async function saveWeight() {
    const w = Number(weightInput);
    if (!(w > 0)) return;
    current.weight_g = w;
    weightInput = '';
    await persist();
  }

  // Fix a mistyped weight on a burst already weighed, without pumping again.
  let editing = $state(null); // burst index
  let editInput = $state('');
  function startEdit(i) {
    editing = i;
    editInput = String(d.bursts[i].weight_g);
  }
  async function saveEdit() {
    const w = Number(editInput);
    if (!(w > 0) || editing == null) return;
    d.bursts[editing].weight_g = w;
    editing = null;
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
    err = null;
    // A burst still pumping must stop with its session: once the draft is
    // gone nothing on this page knows the run is ours, and it would pump on.
    if (burstRunning) {
      working = true;
      try {
        await post(`/api/runs/${current.run_id}/stop`);
      } catch (e) {
        err = { message: `Could not stop the pump: ${e.message}`, hint: null };
        return;
      } finally {
        working = false;
      }
    }
    await del('/api/calibrations/draft').catch(() => {});
    Object.assign(d, blank());
    saved = null;
    err = null;
  }

  // 5.007 min reads "5 min 0.4 s": the real time the burst pumped.
  const minSec = (min) => {
    const tenths = Math.round(min * 600); // whole tenths of a second
    return `${Math.floor(tenths / 600)} min ${num((tenths % 600) / 10, 1)} s`;
  };

  // The balance every second across each burst (journalled by the daemon):
  // the flow as a slope over the steady middle (start and stop transients
  // left out), and how much it changed from the first half to the second.
  // Diagnostic only; the recorded weight stays the before/after difference.
  const SKIP_START_S = 15;
  const SKIP_END_S = 3;
  let profiles = $state({}); // run_id -> { gPerMin, driftPct } | null
  function slope(pts) {
    const n = pts.length;
    if (n < 5) return null;
    const mx = pts.reduce((a, p) => a + p[0], 0) / n;
    const my = pts.reduce((a, p) => a + p[1], 0) / n;
    let sxy = 0;
    let sxx = 0;
    for (const [x, y] of pts) {
      sxy += (x - mx) * (y - my);
      sxx += (x - mx) ** 2;
    }
    return sxx > 0 ? sxy / sxx : null;
  }
  $effect(() => {
    for (const b of d.bursts) {
      if (b.weight_g == null || b.run_id in profiles) continue;
      profiles[b.run_id] = null;
      get(`/api/runs/${b.run_id}/ticks?from=0&to=100000`)
        .then((ticks) => {
          const end = Math.max(...ticks.map((t) => t.elapsed_s));
          const pts = ticks
            .filter((t) => t.weight_g != null && t.elapsed_s >= SKIP_START_S && t.elapsed_s <= end - SKIP_END_S)
            .map((t) => [t.elapsed_s, t.weight_g]);
          // A burst from before the balance was read mid-burst journals one
          // stale weight throughout: nothing to show.
          if (new Set(pts.map((p) => p[1])).size < 3) return;
          const all = slope(pts);
          if (all == null) return;
          const mid = pts.length >> 1;
          const a = slope(pts.slice(0, mid));
          const z = slope(pts.slice(mid));
          profiles[b.run_id] = {
            gPerMin: Math.abs(all) * 60,
            driftPct: a && z ? (100 * (Math.abs(z) - Math.abs(a))) / Math.abs(a) : null,
          };
        })
        .catch(() => {});
    }
  });

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
        internal_ref: (d.internal_ref ?? '').trim() || null,
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
      'id', 'created_at', 'tubing_lot_id', 'internal_ref', 'inner_diameter_mm',
      'outer_diameter_mm', 'control_var', 'setpoint',
      'density_g_per_ml', 'run_1_id', 'run_2_id', 'run_3_id', 'weight_1_g', 'weight_2_g',
      'weight_3_g', 'measured_1_ml_min', 'measured_2_ml_min', 'measured_3_ml_min',
      'mean_measured_ml_min', 'cv_pct', 'c0', 'operator', 'note', 'archived_at',
    ];
    const cell = (v) => {
      const s = v == null ? '' : String(v);
      return /[",\n]/.test(s) ? `"${s.replaceAll('"', '""')}"` : s;
    };
    const lines = history.map((c) =>
      [
        c.id, c.created_at, c.tubing_lot_id, c.internal_ref, c.inner_diameter_mm,
        c.outer_diameter_mm, c.control_var, c.setpoint,
        c.density_g_per_ml, ...c.run_ids, ...c.weights_g, ...c.measured_ml_min,
        c.mean_measured_ml_min, c.cv_pct, c.c0, c.operator, c.note, c.archived_at,
      ].map(cell).join(','),
    );
    const blob = new Blob([[cols.join(','), ...lines].join('\n') + '\n'], { type: 'text/csv' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = 'tubing-calibrations.csv';
    a.click();
    // Not at once: some browsers drop a download whose URL is gone before it starts.
    setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
  }

  const cvBad = (cv) => cv > CV_WARN_PCT;
  const c0Bad = (cv, c0) => cv === 'ml_min' && (c0 < C0_MIN || c0 > C0_MAX);
</script>

<section class="card">
  <div class="card-head">
    <div>
      <div class="eyebrow">Tubing</div>
      <h2>Tubing calibration</h2>
      <p class="sub">
        Three bursts at one setpoint, each weighed: automatically with the feed bottle on the
        connected balance, or by hand. Required before a gravimetric trim in rpm, recommended in
        ml/min. Calibrate a tube before it is autoclaved and installed.
      </p>
    </div>
  </div>

  {#if !loaded}
    <p class="muted">Loading…</p>
  {:else}
    <div class="group">
      <div class="eyebrow">Tube</div>
      <div class="grid">
        <label class="field"><span>Tubing lot</span>
          <input type="text" bind:value={d.tubing_lot_id} disabled={locked} onchange={persist} placeholder="e.g. LOT-2409" />
        </label>
        <label class="field"><span>Internal ref (optional)</span>
          <input type="text" bind:value={d.internal_ref} disabled={locked} onchange={persist} />
        </label>
        <label class="field"><span>Inner Ø (mm)</span>
          <input type="number" step="0.1" min="0" bind:value={d.inner_diameter_mm} disabled={locked} onchange={persist} />
        </label>
        <label class="field"><span>Outer Ø (mm)</span>
          <input type="number" step="0.1" min="0" bind:value={d.outer_diameter_mm} disabled={locked} onchange={persist} />
        </label>
      </div>
      {#if Number(d.inner_diameter_mm) > 0 && Number(d.outer_diameter_mm) > 0 && Number(d.outer_diameter_mm) <= Number(d.inner_diameter_mm)}
        <p class="warn">The outer Ø must be larger than the inner Ø.</p>
      {/if}
    </div>

    <div class="group">
      <div class="eyebrow">Pumping</div>
      <div class="grid">
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
        <label class="field"><span>Calibration time (min)</span>
          <input type="number" step="0.5" min="0" bind:value={d.target_min} onchange={persist} />
        </label>
      </div>
      <p class="help">
        The pump runs each burst for the calibration time, then stops by itself (you can stop it
        earlier). The flow is computed from the time it really pumped.
      </p>
    </div>

    <div class="group">
      <div class="eyebrow">Weighing</div>
      <div class="grid">
        <label class="field"><span>Liquid density (g/mL)</span>
          <input type="number" step="0.001" min="0" bind:value={d.density_g_per_ml} disabled={locked} onchange={persist} />
        </label>
        <label class="field"><span>Operator (optional)</span>
          <input type="text" bind:value={d.operator} onchange={persist} />
        </label>
      </div>
      <p class="help">
        Density of the liquid pumped during the calibration, to turn the weighed grams into mL.
        Water or dilute media ≈ 1.00 (not the concentration: 5 g/L is still ≈ 1.00 g/mL).
      </p>
    </div>

    <div class="group">
      <div class="eyebrow">Bursts</div>
      <ol class="bursts">
        {#each [0, 1, 2] as i}
          {@const b = d.bursts[i]}
          <li class:done={b?.weight_g != null} title={b ? `Run #${b.run_id}` : undefined}>
            <span class="n mono">{i + 1}/3</span>
            {#if !b}
              <span class="muted">not started</span>
            {:else if active?.run_id === b.run_id}
              <span>pumping{remainingS != null ? `, ${dur(remainingS)} left` : ''}</span>
              {#if collectedG != null}
                <span class="drawn mono" title="Balance change since the burst started, live">{num(collectedG, 2)} g drawn</span>
              {/if}
            {:else if b.weight_g == null}
              <span>
                stopped: {b.start_g != null
                  ? 'take the balance reading'
                  : 'weigh the collected feed'}
              </span>
            {:else if editing === i}
              <input class="edit-w" type="number" step="0.01" min="0" bind:value={editInput}
                onkeydown={(e) => { if (e.key === 'Enter') saveEdit(); if (e.key === 'Escape') editing = null; }} />
              <span>g</span>
              <button class="btn-ghost small" disabled={!(Number(editInput) > 0)} onclick={saveEdit}>Save</button>
              <button class="btn-ghost small" onclick={() => (editing = null)}>Cancel</button>
            {:else}
              <span class="mono">{num(b.weight_g, 2)} g{durations[b.run_id] ? ` in ${minSec(durations[b.run_id])}` : ''}</span>
              {#if profiles[b.run_id]}
                {@const pr = profiles[b.run_id]}
                <span class="profile mono" title="Balance read every second during the burst: flow from the slope (first {SKIP_START_S} s and last {SKIP_END_S} s left out), and its change from the first half of the burst to the second.">
                  slope {num(pr.gPerMin, 3)} g/min{pr.driftPct != null ? ` | ${pr.driftPct >= 0 ? '+' : ''}${num(pr.driftPct, 1)} % within` : ''}
                </span>
              {/if}
              <button class="btn-ghost small" disabled={working} onclick={() => startEdit(i)}>Edit</button>
            {/if}
          </li>
        {/each}
      </ol>

      {#if otherRunActive}
        <p class="warn">
          {active.kind === 'calibration'
            ? `Calibration burst run #${active.run_id} is still pumping, outside this session.`
            : 'Another run is active. Stop it before starting a burst.'}
          {#if active.kind === 'calibration'}
            <button class="btn-ghost" disabled={working} onclick={stopOther}>Stop it</button>
          {/if}
        </p>
      {/if}

      {#if autoElsewhere}
        <div class="actions">
          <span class="live mono">Automatic mode is driving this calibration from another window.</span>
          <button class="btn-ghost" onclick={() => setAuto(true)}>Take over here</button>
          <button class="btn-ghost" onclick={() => setAuto(false)}>Stop automatic</button>
        </div>
      {:else if d.auto}
        <div class="actions">
          <span class="live mono">Automatic: {autoStep}</span>
          <button class="btn-ghost" onclick={() => setAuto(false)}>Stop automatic</button>
        </div>
      {:else if canAuto && !burstRunning}
        <div class="actions">
          <button class="btn-primary" disabled={!formOk || working} onclick={() => setAuto(true)}>
            Run {d.bursts.length ? 'the remaining bursts' : 'all 3 bursts'} automatically
          </button>
          <span class="help inline">Balance under the feed bottle: each burst starts, stops and is weighed by itself.</span>
        </div>
      {/if}

      <div class="actions">
        {#if burstRunning}
          <button class="btn-primary" disabled={working} onclick={stopBurst}>Stop and weigh</button>
        {:else if needsWeight && !d.auto}
          {#if collectedG != null}
            <button class="btn-primary" disabled={!scaleStable || !(collectedG > 0)} onclick={saveBalanceReading}>
              Take balance reading: {num(collectedG, 2)} g
            </button>
            <span class="live mono" class:settling={!scaleStable}>
              {scaleStable ? 'stable' : 'settling…'} | {num(scaleW, 2)} g now, {num(current.start_g, 2)} g before
            </span>
          {/if}
          <label class="field weigh"><span>Collected weight (g)</span>
            <input type="number" step="0.01" min="0" bind:value={weightInput} onkeydown={(e) => e.key === 'Enter' && saveWeight()} />
          </label>
          <button class={collectedG != null ? 'btn-ghost' : 'btn-primary'} disabled={!(Number(weightInput) > 0)} onclick={saveWeight}>
            Save typed weight
          </button>
        {:else if d.bursts.length < 3 && !d.auto}
          <button class="btn-primary" disabled={!canStartBurst} onclick={startBurst}>
            {working ? 'Starting…' : `Start burst ${d.bursts.length + 1}/3`}
          </button>
          {#if scaleW != null}
            <span class="live mono" class:settling={!scaleStable}>
              balance {num(scaleW, 2)} g{scaleStable ? '' : ' (settling)'}: taken as the starting weight
            </span>
          {/if}
        {/if}
        {#if current && !burstRunning}
          <button class="btn-ghost" onclick={redoLast}>Redo burst {d.bursts.length}</button>
        {/if}
        {#if confirmDiscard}
          <span class="confirm">
            Discard this session?{burstRunning ? ' The pump stops now.' : ''} The bursts stay recorded.
          </span>
          <button class="btn-danger" disabled={working} onclick={discard}>{burstRunning ? 'Stop and discard' : 'Discard'}</button>
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
          <div>flows {shown.measured_ml_min.map((m) => num(m, 3)).join(' | ')} ml/min</div>
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
        <span class="head-actions">
          {#if archivedCount}
            <label class="toggle"><input type="checkbox" bind:checked={showArchived} /> Show archived ({archivedCount})</label>
          {/if}
          {#if history.length}<button class="btn-ghost" onclick={exportCsv}>Export CSV</button>{/if}
        </span>
      </div>
      {#if listed.length}
        <table class="hist mono">
          <thead>
            <tr><th>#</th><th>date</th><th>lot</th><th>internal ref</th><th>Ø int / ext (mm)</th><th>setpoint</th><th>mean ml/min</th><th>CV %</th><th>c₀</th><th></th></tr>
          </thead>
          <tbody>
            {#each listed as c (c.id)}
              <tr class:archived={c.archived_at}>
                <td>{c.id}</td>
                <td>{stamp(c.created_at)}</td>
                <td>{c.tubing_lot_id}</td>
                <td>{c.internal_ref ?? ''}</td>
                <td>{num(c.inner_diameter_mm, 1)} / {num(c.outer_diameter_mm, 1)}</td>
                <td>{num(c.setpoint, digitsFor(c.control_var))} {unitFor(c.control_var)}</td>
                <td>{num(c.mean_measured_ml_min, 3)}</td>
                <td class:bad={cvBad(c.cv_pct)}>{num(c.cv_pct, 2)}</td>
                <td class:bad={c0Bad(c.control_var, c.c0)}>{c.control_var === 'ml_min' ? num(c.c0, 4) : '·'}</td>
                <td class="row-action">
                  {#if c.archived_at}
                    <span class="tag" title="Archived {stamp(c.archived_at)}">archived</span>
                    <button class="btn-ghost" disabled={archiving === c.id} onclick={() => setArchived(c, false)}>Restore</button>
                  {:else}
                    <button class="btn-ghost" disabled={archiving === c.id} onclick={() => setArchived(c, true)}>Archive</button>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
        <p class="help">
          Archive a tube you no longer use: it leaves the New run list but stays on record, with
          the runs that used it. Restore brings it back.
        </p>
      {:else if history.length}
        <p class="muted">All calibrations are archived.</p>
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
  .bursts li { align-items: center; }
  .edit-w { width: 90px; }
  .btn-ghost.small { padding: 2px var(--s-2); font-size: 12px; }
  .actions { display: flex; gap: var(--s-3); align-items: flex-end; flex-wrap: wrap; margin-top: var(--s-3); }
  .weigh { width: 180px; }
  .drawn { font-weight: 600; color: var(--teal-700); }
  .live { font-size: 12px; color: var(--green-600); align-self: center; }
  .live.settling { color: var(--muted); }
  .help.inline { margin: 0; align-self: center; }
  .profile { font-size: 12px; color: var(--muted); }
  .confirm { font-size: 13px; align-self: center; }
  .warn { font-size: 13px; color: var(--danger); margin: var(--s-3) 0 0; }
  .help { font-size: 12px; color: var(--muted); margin: var(--s-3) 0 0; max-width: 70ch; line-height: 1.4; }
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
  .hist tr.archived td { color: var(--muted); }
  .hist .row-action { text-align: right; white-space: nowrap; }
  .hist .row-action .btn-ghost { padding: 2px var(--s-2); font-size: 12px; }
  .tag { font-size: 11px; margin-right: var(--s-2); color: var(--muted); }
  /* inside an eyebrow line: back to normal text for the controls */
  .head-actions {
    display: flex; align-items: center; gap: var(--s-3);
    text-transform: none; letter-spacing: normal; font-size: 14px; font-weight: 400; color: var(--ink);
  }
  .toggle { font-size: 12px; font-weight: 400; color: var(--muted); display: flex; align-items: center; gap: var(--s-1); }
  @media (max-width: 720px) {
    .hist { display: block; overflow-x: auto; }
  }
</style>
