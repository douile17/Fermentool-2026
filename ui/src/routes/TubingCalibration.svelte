<script>
  import { untrack } from 'svelte';
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
  import { modelOf, setpointStep } from '../lib/pumps.js';
  import ErrorText from '../components/ErrorText.svelte';
  import Hint from '../components/Hint.svelte';
  import PumpHead from '../components/PumpHead.svelte';
  import Icon from '../components/Icon.svelte';
  import Steps from '../components/Steps.svelte';
  import MeasurementChart from '../components/MeasurementChart.svelte';
  import {
    measurementProfile, defaultMeasurement, pushStable, stableMean, redoAllSummary, withoutMeasurements, SKIP_START_S, SKIP_END_S,
  } from '../lib/measurement.js';
  import { expectedMlMin, stallCheckAtS, burstTrouble } from '../lib/stall.js';
  import { inches, sizeParts, boreClass, parseSize, COMMON_SIZES, MM_PER_IN, KNOWN_BRANDS } from '../lib/tubesize.js';

  // Mirrors trim::CALIBRATION_CV_WARN_PCT and the trim bounds (Settings,
  // correction limit): warnings only.
  const CV_WARN_PCT = 5;
  let trimLimit = $state(25);
  const C0_MAX = $derived(Number((1 + trimLimit / 100).toFixed(2)));
  const C0_MIN = $derived(Number((1 / (1 + trimLimit / 100)).toFixed(2)));

  const blank = () => ({
    tubing_lot_id: '',
    brand: '',
    product_ref: '',
    internal_ref: '',
    inner_diameter_mm: null,
    outer_diameter_mm: null,
    // Always rpm for a new calibration: an ml/min run is then driven in rpm
    // with the tube's ml per rpm, and the pump's own head/tubing table never
    // matters (CLAUDE.md, Conventions).
    control_var: 'rpm',
    // No default: the tube is often mounted the other way round, and bursts
    // pumped the wrong way are lost. `direction_set` marks a real choice.
    direction: null,
    direction_set: false,
    // No default either: the speed is chosen for the tube on the pump.
    // `setpoint_set` marks a typed value, as `direction_set` a picked one.
    setpoint: null,
    setpoint_set: false,
    density_g_per_ml: 1.0,
    target_min: 5,
    operator: '',
    note: '',
    // run the three bursts and their weighing without a click (balance needed)
    auto: false,
    // the page (tab, window) driving the automatic mode: only that one acts
    auto_owner: null,
    // 'auto' | 'manual', chosen in the settings step (null: auto when a
    // balance is connected)
    weigh_mode: null,
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
  let runStarts = $state({}); // run_id -> started_at, for the detail
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
        // A session not started yet becomes rpm too; one with bursts keeps its
        // control, fixed by them.
        if (!d.bursts.length && d.control_var !== 'rpm') {
          d.control_var = 'rpm';
          persist();
        }
        // A direction nobody picked (an older draft's default) is no choice.
        if (!d.bursts.length && !d.direction_set) d.direction = null;
        // An older draft's default speed (100) is no choice either.
        if (!d.bursts.length && !d.setpoint_set) d.setpoint = null;
        d.bursts.forEach((b, i) => (b.slot ??= i));
        d.bursts.sort((a, b) => a.slot - b.slot);
        syncInches();
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
  // Each burst keeps its card (`slot` 0..2): pumping one again refills its
  // own card, the others stay where they are. `current` is the burst not
  // weighed yet, pumping or waiting to be weighed; null when all are weighed.
  const current = $derived(d.bursts.find((b) => b.weight_g == null) ?? null);
  const bySlot = (i) => d.bursts.find((b) => b.slot === i) ?? null;
  const freeSlot = $derived([0, 1, 2].find((i) => !bySlot(i)) ?? null);
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

  const directionOk = $derived(d.direction === 'cw' || d.direction === 'ccw');
  const formOk = $derived(
    directionOk &&
      d.tubing_lot_id.trim() !== '' &&
      (d.product_ref ?? '').trim() !== '' &&
      (d.brand ?? '').trim() !== '' &&
      Number(d.inner_diameter_mm) > 0 &&
      Number(d.outer_diameter_mm) > Number(d.inner_diameter_mm) &&
      Number(d.setpoint) > 0 &&
      Number(d.density_g_per_ml) > 0 &&
      Number(d.target_min) > 0,
  );
  // The burst with an alarm (CHECK on its card), if any: nothing goes on
  // until it is pumped again, or its alarm waved off with "Keep going".
  const alarmed = $derived(d.bursts.find((b) => b.alarm) ?? null);
  const canStartBurst = $derived(
    loaded &&
      formOk &&
      !working &&
      !active &&
      canStartRun(app.status) &&
      d.bursts.length < 3 &&
      (current == null || current.weight_g != null) &&
      !alarmed,
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
    const slot = freeSlot ?? d.bursts.length;
    const n = slot + 1;
    const setpoint = Number(d.setpoint);
    const lim = d.control_var === 'ml_min' ? FLOW_LIMITS : { ...RPM_LIMITS, max: app.status?.pump_max_rpm ?? RPM_LIMITS.max };
    // before the pump moves: the mean of the stable readings, as at the end
    const startG = avgW ? Number(avgW.g.toFixed(2)) : scaleW;
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
      d.bursts.push({ run_id: r.run_id, weight_g: null, start_g: startG, slot });
      d.bursts.sort((a, b) => a.slot - b.slot);
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

  // A burst cut short is never weighed: a part burst reads as a different
  // flow. Restarting stops the pump, drops the burst unweighed and pumps its
  // card again (by itself in automatic weighing, once the balance is still).
  async function restartBurst() {
    const b = current;
    if (!b) return;
    err = null;
    working = true;
    try {
      await post(`/api/runs/${b.run_id}/stop`);
      d.bursts.splice(d.bursts.indexOf(b), 1);
      await persist();
    } catch (e) {
      err = { message: e.message, hint: null };
      return;
    } finally {
      working = false;
    }
    if (weighMode === 'auto' && autoAvailable && !alarmed) await setAuto(true);
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
    if (!(weighG > 0)) return;
    current.weight_g = Number(weighG.toFixed(2));
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
    if (!d.auto && !needsWeight) return;
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
  // The weighing is the mean of the last 5 s of stable readings
  // (lib/measurement.js), at the start as at the end: one reading on a
  // balance flickering 27.4 / 27.5 / 27.6 g was whichever came at the click.
  // Nothing while a measurement pumps, nor in the AFTER_STOP_S after it.
  let stableBuf = $state([]);
  $effect(() => {
    const s = app.status; // every status pushed, about once a second
    if (!s) return;
    untrack(() => {
      const t = Date.now();
      if (burstRunning || (stoppedAt != null && t < stoppedAt + AFTER_STOP_S * 1000)) stableBuf = [];
      else stableBuf = pushStable(stableBuf, { t, w: s.scale_weight_g, stable: s.scale_stable === true });
    });
  });
  const avgW = $derived(stableMean(stableBuf));
  // What a weighing now would record: the mean against the start weight.
  const weighG = $derived(current?.start_g != null && avgW ? Math.abs(avgW.g - current.start_g) : collectedG);
  const canAuto = $derived(scaleW != null && !complete && !otherRunActive && (current == null || current.start_g != null || current.weight_g != null));
  const autoStep = $derived(
    burstRunning
      ? `measurement ${(current?.slot ?? 0) + 1}/3 pumping`
      : needsWeight
        ? 'settling, then weighing'
        : `measurement ${(freeSlot ?? 0) + 1}/3 starts when the balance is still`,
  );

  async function setAuto(on) {
    d.auto = on;
    d.auto_owner = on ? PAGE : null;
    err = null;
    await persist();
  }

  $effect(() => {
    if (!autoHere || working || burstRunning) return;
    if (alarmed) {
      setAuto(false);
      return;
    }
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
        markAlarm('dry');
        err = {
          message: `The balance moved by ${num(collectedG ?? 0, 2)} g over the measurement: automatic mode stopped.`,
          hint: 'Is the feed bottle on the balance, and the line drawing from it? Redo this measurement once it is.',
        };
        d.auto = false;
        d.auto_owner = null;
        persist();
      }
    } else if (canStartBurst) {
      startBurst();
    }
  });

  // In automatic weighing a burst is weighed by itself once the balance is
  // still, whether or not the sequence runs (a burst started by hand, the
  // sequence stopped meanwhile): there is never a button to press for it.
  $effect(() => {
    if (d.auto || working || burstRunning || !needsWeight || current.alarm) return;
    if (steadyS < STEADY_S) return;
    if (current.run_id === startedHere && !seenRunning.has(current.run_id)) return;
    if (weighG != null && weighG >= MIN_COLLECTED_G) {
      if (weighMode === 'auto') saveBalanceReading();
    } else if (weighG != null) {
      markAlarm('dry');
    }
  });

  // A burst that went wrong keeps the mark on its card (and in the draft):
  // 'still' / 'stopped' / 'reversed' from the balance check while it ran,
  // 'dry' when it came out with almost nothing. That burst can be pumped
  // again at once; the others once all three are done.
  function markAlarm(kind) {
    if (!current || current.alarm) return;
    current.alarm = kind;
    persist();
  }

  let weightInput = $state('');
  async function saveWeight() {
    const w = Number(weightInput);
    if (!(w > 0)) return;
    current.weight_g = w;
    weightInput = '';
    await persist();
  }

  // Fix a mistyped weight on a burst already weighed, without pumping again.
  let editing = $state(null); // card (slot)
  let editInput = $state('');
  function startEdit(i) {
    editing = i;
    editInput = String(bySlot(i).weight_g);
  }
  async function saveEdit() {
    const w = Number(editInput);
    if (!(w > 0) || editing == null || !bySlot(editing)) return;
    bySlot(editing).weight_g = w;
    editing = null;
    await persist();
  }

  // Any burst can be pumped again, not only the last: the three are three
  // samples of one flow, in no order, so redoing burst 1 drops its weighing
  // and the next burst pumped takes its place among the three. A weighed one
  // asks a second click first (its weighing is lost; its run stays in History).
  let confirmRedo = $state(null); // card (slot)
  async function redoBurst(i) {
    const b = bySlot(i);
    if (!b) return;
    // A good weighing is asked about first; one with an alarm is bad anyway.
    if (b.weight_g != null && !b.alarm && confirmRedo !== i) {
      confirmRedo = i;
      return;
    }
    confirmRedo = null;
    if (editing === i) editing = null;
    d.bursts.splice(d.bursts.indexOf(b), 1);
    await persist();
    // In automatic weighing the sequence picks up from here, driven by this
    // page (it takes over from a window closed or reloaded meanwhile).
    if (weighMode === 'auto' && autoAvailable && !alarmed) await setAuto(true);
  }

  // Redo all 3: every measurement dropped, the tube and the pump setup kept
  // (Abandon clears those too). Asked first; never while one pumps. Their
  // runs stay in the journal, as with one measurement redone.
  let confirmAll = $state(false);
  const allSummary = $derived(redoAllSummary(d.bursts));
  async function redoAll() {
    if (burstRunning || !d.bursts.length) return;
    confirmAll = false;
    confirmRedo = null;
    editing = null;
    picked = 0;
    Object.assign(d, withoutMeasurements(d));
    await persist();
    // In automatic weighing the sequence starts over from measurement 1.
    if (weighMode === 'auto' && autoAvailable) await setAuto(true);
  }

  // Two-step, in-page: a native confirm() dialog is not something the
  // desktop webview can be relied on to show.
  let confirmDiscard = $state(false);
  // Something typed or pumped worth asking before clearing.
  const abandonable = $derived(locked || !!d.tubing_lot_id?.trim());
  // Redo: any measurement pumped, at any time, never while one pumps.
  const redoable = (i) => !burstRunning && !!bySlot(i);
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
        return false;
      } finally {
        working = false;
      }
    }
    await del('/api/calibrations/draft').catch(() => {});
    Object.assign(d, blank());
    syncInches();
    saved = null;
    err = null;
    return true;
  }

  // Leaving this page for another clears the calibration, once confirmed
  // (App asks, see app.leaveGuard). Only when something was entered; a
  // calibration just recorded has nothing left to lose.
  $effect(() => {
    const guard = {
      active: () =>
        !saved &&
        (abandonable || !!d.brand?.trim() || !!d.product_ref?.trim() || Number(d.inner_diameter_mm) > 0 || Number(d.outer_diameter_mm) > 0),
      get title() {
        return 'Leave the tubing calibration?';
      },
      get lines() {
        const n = d.bursts.length;
        return [
          ...(burstRunning ? ['The measurement in progress stops the pump.'] : []),
          n ? `The tube, the setpoint and the ${n} measurement${n === 1 ? '' : 's'} so far are cleared.` : 'What you entered is cleared.',
          ...(n ? ['The measurements already pumped stay in the journal.'] : []),
        ];
      },
      leave: () => discard(),
    };
    app.leaveGuard = guard;
    return () => {
      if (app.leaveGuard === guard) app.leaveGuard = null;
    };
  });

  // 5.007 min reads "5 min 0.4 s": the real time the burst pumped.
  const minSec = (min) => {
    const tenths = Math.round(min * 600); // whole tenths of a second
    return `${Math.floor(tenths / 600)} min ${num((tenths % 600) / 10, 1)} s`;
  };

  // The balance every second across each measurement (journalled by the
  // daemon): its curve, the flow as a slope over the steady middle and how
  // it changed from the first half to the second (lib/measurement.js).
  // Diagnostic only; the recorded weight stays the before/after difference.
  let profiles = $state({}); // run_id -> measurementProfile | null
  $effect(() => {
    for (const b of d.bursts) {
      if (b.weight_g == null || b.run_id in profiles) continue;
      profiles[b.run_id] = null;
      get(`/api/runs/${b.run_id}/ticks?from=0&to=100000`)
        .then((ticks) => (profiles[b.run_id] = measurementProfile(ticks, resG)))
        .catch(() => {});
    }
  });

  // Real measurement durations, and when each started.
  $effect(() => {
    for (const b of d.bursts) {
      if (b.weight_g == null || durations[b.run_id] != null) continue;
      get(`/api/runs/${b.run_id}`)
        .then((run) => {
          if (run?.started_at) runStarts[b.run_id] = run.started_at;
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
        product_ref: (d.product_ref ?? '').trim() || null,
        brand: (d.brand ?? '').trim() || null,
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
      'id', 'created_at', 'tubing_lot_id', 'brand', 'product_ref', 'internal_ref', 'inner_diameter_mm',
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
        c.id, c.created_at, c.tubing_lot_id, c.brand, c.product_ref, c.internal_ref, c.inner_diameter_mm,
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

  // --- The page as four steps, in the order of the job (tube, settings,
  // bursts, result), with the session summed up beside them.
  let step = $state(1);
  // The tube fields turn orange once left empty, not before anything is
  // typed: marked from the start they read as errors (UX audit, 19 Oct).
  let left = $state({});
  let stepPicked = false;
  $effect(() => {
    if (!loaded || stepPicked) return;
    stepPicked = true;
    step = complete ? 4 : d.bursts.length ? 3 : 1;
  });
  // A burst pumping (started here, by automatic mode or elsewhere) is watched
  // from the bursts step.
  $effect(() => {
    if (burstRunning && step !== 3) step = 3;
  });
  const busy = $derived(burstRunning || d.auto);
  const tubeOk = $derived(
    d.tubing_lot_id.trim() !== '' && (d.product_ref ?? '').trim() !== '' && (d.brand ?? '').trim() !== '' && Number(d.inner_diameter_mm) > 0 && Number(d.outer_diameter_mm) > Number(d.inner_diameter_mm),
  );
  const stepDefs = $derived([
    {
      n: 1,
      label: 'Tube',
      sub: d.tubing_lot_id.trim() ? `${d.tubing_lot_id.trim()}, ${d.inner_diameter_mm ?? '?'} / ${d.outer_diameter_mm ?? '?'} mm` : 'lot and diameters',
      reach: true,
      done: tubeOk || !!saved,
    },
    {
      n: 2,
      label: 'Pump setup',
      sub: `${Number(d.setpoint) > 0 ? `${d.setpoint} ${unit}` : `${unit} to set`}, ${d.direction ?? 'direction to choose'}, ${d.target_min} min`,
      reach: true,
      // green as soon as its own fields are filled, not once a burst ran
      done: (directionOk && Number(d.setpoint) > 0 && Number(d.target_min) > 0 && Number(d.density_g_per_ml) > 0) || !!saved,
    },
    { n: 3, label: 'Measurements', sub: `${d.bursts.filter((b) => b.weight_g != null).length} of 3 weighed`, reach: formOk, done: complete || !!saved },
    { n: 4, label: 'Result', sub: saved ? `recorded, #${saved.id}` : complete ? 'ready to record' : 'after the measurements', reach: complete || !!saved, done: !!saved },
  ]);


  // Diameters typed as the bag prints them, in mm or in inches (0.063, .062,
  // 1/16); always stored in mm. The entry starts in the unit Settings, Display
  // leads with. In inches the typed text is kept as typed, only its mm value
  // goes into the draft.
  let diaUnit = $state(null);
  const unitNow = $derived(diaUnit ?? app.tubeSizes);
  let inchText = $state({ inner: '', outer: '' });
  const inchOf = (mm) => (mm > 0 ? inches(mm).replace('″', '') : '');
  function syncInches() {
    inchText = { inner: inchOf(d.inner_diameter_mm), outer: inchOf(d.outer_diameter_mm) };
  }
  function setDiaUnit(u) {
    diaUnit = u;
    syncInches();
  }
  function typeInches(which, text) {
    inchText[which] = text;
    const v = parseSize(text);
    d[`${which}_diameter_mm`] = v > 0 ? Math.round(v * MM_PER_IN * 100) / 100 : null;
  }
  // A usual size fills in what its chip shows: in mm the metric name off the
  // bag (0.8 / 1.6), in inches their exact value (0.79 / 1.59 mm). Both file
  // under the same bore.
  function useSize([inner, outer]) {
    const places = unitNow === 'mm' ? 10 : 100;
    d.inner_diameter_mm = Math.round(inner * MM_PER_IN * places) / places;
    d.outer_diameter_mm = Math.round(outer * MM_PER_IN * places) / places;
    syncInches();
    persist();
  }
  // The same brand, reference and lot calibrated before: said once, since the new
  // calibration then takes over from that one in New run.
  const sameTube = $derived.by(() => {
    const lot = d.tubing_lot_id?.trim().toLowerCase();
    if (!lot || saved) return null;
    const norm = (v) => (v ?? '').trim().toLowerCase();
    return (
      history.find(
        (c) => !c.archived_at && norm(c.tubing_lot_id) === lot && norm(c.product_ref) === norm(d.product_ref) && norm(c.brand) === norm(d.brand),
      ) ?? null
    );
  });
  // The tube's section to scale: swapped diameters show at a glance.
  const tubeSvg = $derived.by(() => {
    const i = Number(d.inner_diameter_mm) || 0;
    const o = Number(d.outer_diameter_mm) || 0;
    const big = Math.max(i, o, 0.1);
    const ro = Math.max(4, (52 * o) / big);
    return { ro, ri: Math.max(2, Math.min(ro - 2, (52 * i) / big)), wall: o > i && i > 0 ? (o - i) / 2 : null };
  });
  // Lot and coherent diameters: enough to go on to the settings.
  // Brand and reference name the tube; both are asked before any burst.
  const namesOk = $derived((d.brand ?? '').trim() !== '' && (d.product_ref ?? '').trim() !== '');
  // Brands to pick from: the ones this PC used, then the usual makers.
  const brands = $derived(
    [...new Set([...KNOWN_BRANDS, ...history.map((c) => c.brand?.trim()).filter(Boolean)])].sort((a, b) =>
      a.localeCompare(b, undefined, { sensitivity: 'base' }),
    ),
  );
  // "Custom..." opens a box to type a brand the list lacks; a draft holding
  // one already opens on it.
  let customBrand = $state(false);
  const brandCustom = $derived(customBrand || (!!d.brand?.trim() && !brands.includes(d.brand.trim())));
  let brandBox = $state();
  function pickBrand(v) {
    if (v === '__custom') {
      customBrand = true;
      d.brand = '';
      requestAnimationFrame(() => brandBox?.focus());
      return;
    }
    customBrand = false;
    d.brand = v;
    persist();
  }
  const diaBad = $derived(
    Number(d.inner_diameter_mm) > 0 && Number(d.outer_diameter_mm) > 0 && Number(d.outer_diameter_mm) <= Number(d.inner_diameter_mm),
  );

  const totalMin = $derived(3 * (Number(d.target_min) || 0));

  // Automatic needs the balance; the choice is kept in the draft.
  const autoAvailable = $derived(scaleW != null);
  // The mode shown is the mode used: automatic unless Manual was picked. It
  // never flips by itself when the balance drops (it used to, silently, and a
  // burst then started by hand with no starting weight).
  const weighMode = $derived(d.weigh_mode ?? 'auto');
  // A click on the greyed zone shakes the banner again: the way out is there.
  let nudges = $state(0);

  // Automatic chosen but no balance: nothing starts until it is back or the
  // operator switches to Manual.
  const autoBlocked = $derived(weighMode === 'auto' && !autoAvailable);
  function setWeighMode(m) {
    d.weigh_mode = m;
    persist();
  }

  // The balance's step: what a burst's weight is known to (0.1 g until the
  // daemon has a reading, and for weights typed by hand).
  const resG = $derived(app.status?.scale_resolution_g ?? 0.1);
  const wDig = $derived(Math.max(0, Math.round(-Math.log10(resG))));
  // Decimals worth showing for a figure computed from a weight: its last
  // digit is about one balance step of it. 4.1 g over 1 min 0.07 s is
  // 4.095 ml/min on paper, but only known to 0.1.
  function sig(value, weightG) {
    if (!(value > 0) || !(weightG > 0) || !Number.isFinite(weightG)) return 3;
    return Math.min(4, Math.max(0, Math.round(-Math.log10((value * resG) / weightG))));
  }

  // Roughly what one burst collects, to warn when a balance step is a large
  // share of it. In rpm, the tube's ml per rpm comes from an earlier
  // calibration of the same bore; none, no estimate.
  const STEP_LIMIT_PCT = 0.5;
  const burstEstimate = $derived.by(() => {
    const sp = Number(d.setpoint), min = Number(d.target_min), rho = Number(d.density_g_per_ml);
    if (!(sp > 0 && min > 0 && rho > 0)) return null;
    let mlMin = null;
    if (d.control_var === 'ml_min') mlMin = sp;
    else {
      const bore = (h) => h.control_var === 'rpm' && Number(h.inner_diameter_mm) === Number(d.inner_diameter_mm) && h.setpoint > 0;
      const c = history.find((h) => bore(h) && h.tubing_lot_id === d.tubing_lot_id) ?? history.find(bore);
      if (c) mlMin = (sp * c.mean_measured_ml_min) / c.setpoint;
    }
    if (!(mlMin > 0)) return null;
    const g = mlMin * min * rho;
    const goodMin = Math.ceil(resG / (STEP_LIMIT_PCT / 100) / (mlMin * rho));
    return { g, stepPct: (100 * resG) / g, goodMin };
  });

  // A burst whose balance does not move (lib/stall.js): said while it runs,
  // once even half the expected flow would have moved the balance well past
  // its noise. Slow lines are checked late or only at the end, never early.
  const stallMlPerRpm = $derived.by(() => {
    if (d.control_var !== 'rpm') return null;
    const bore = (h) => h.control_var === 'rpm' && Number(h.inner_diameter_mm) === Number(d.inner_diameter_mm) && h.setpoint > 0;
    const c = history.find((h) => bore(h) && h.tubing_lot_id === d.tubing_lot_id) ?? history.find(bore);
    return c ? c.mean_measured_ml_min / c.setpoint : null;
  });
  // The flow this burst should give, and whether it comes from a real
  // calibration (or an ml/min setpoint) rather than the bore's rough figure.
  const burstMlMin = $derived(
    expectedMlMin({
      controlVar: d.control_var,
      setpoint: Number(d.setpoint),
      mlPerRpm: stallMlPerRpm,
      innerMm: Number(d.inner_diameter_mm),
    }),
  );
  const flowTrusted = $derived(d.control_var === 'ml_min' || stallMlPerRpm != null);
  const stallCheckAt = $derived(
    burstRunning && active
      ? stallCheckAtS({
          mlMin: burstMlMin,
          densityGPerMl: Number(d.density_g_per_ml),
          resG,
          burstS: active.duration_s,
        })
      : null,
  );
  const burstElapsedS = $derived(burstRunning && active ? (now - Date.parse(active.started_at)) / 1000 : null);
  // The balance's readings across the running burst, from the one taken just
  // before it: the checks look at how the weight moves, not only how far.
  // One reading per tick of the burst's clock (`now`, twice a second), so a
  // weight that does not move still counts as readings. The list itself is
  // written untracked: it is this effect's output, not one of its inputs.
  let burstSamples = $state.raw({ runId: null, list: [] });
  $effect(() => {
    const at = now;
    const w = scaleW;
    if (!burstRunning || !active || current?.start_g == null || w == null) return;
    const runId = current.run_id;
    const t = (at - Date.parse(active.started_at)) / 1000;
    untrack(() => {
      const prev = burstSamples.runId === runId ? burstSamples.list : [{ t: 0, w: current.start_g }];
      // one a second is plenty, and keeps an hour's burst to 3600 readings
      if (t >= prev[prev.length - 1].t + 1) burstSamples = { runId, list: [...prev, { t, w }] };
      else if (burstSamples.runId !== runId) burstSamples = { runId, list: prev };
    });
  });
  let stallKept = $state([]); // `${run id}:${trouble}` the operator said to keep going on
  const trouble = $derived.by(() => {
    if (!burstRunning || burstSamples.runId !== current?.run_id) return null;
    const t = burstTrouble({
      samples: burstSamples.list,
      checkAtS: stallCheckAt,
      resG,
      mlMin: burstMlMin,
      densityGPerMl: Number(d.density_g_per_ml),
      trusted: flowTrusted,
    });
    return t && !stallKept.includes(`${current.run_id}:${t}`) ? t : null;
  });
  $effect(() => {
    if (trouble) untrack(() => markAlarm(trouble));
  });
  function keepGoing() {
    stallKept = [...stallKept, `${current.run_id}:${trouble}`];
    if (current.alarm) {
      current.alarm = null;
      persist();
    }
  }
  const ALARM_TEXT = {
    still: 'The balance did not move during this measurement.',
    stopped: 'The balance stopped moving during this measurement.',
    reversed: 'The weight went back the wrong way during this measurement.',
    dry: 'This measurement came out with almost nothing on the balance.',
    slow: 'The flow was far below what this tube should give.',
  };
  const TROUBLE = {
    still: 'The balance has not moved since the measurement started',
    stopped: 'The balance has stopped moving',
    reversed: 'The weight is going back the wrong way',
    slow: 'The flow is far below what this tube should give',
  };
  async function stopStalled() {
    if (d.auto) await setAuto(false);
    await stopBurst();
  }

  // Each burst as a card: waiting, pumping, to weigh, weighed.
  const burstFlow = (b) =>
    b?.weight_g != null && durations[b.run_id] > 0 ? b.weight_g / (Number(d.density_g_per_ml) * durations[b.run_id]) : null;
  const cards = $derived(
    [0, 1, 2].map((i) => {
      const b = bySlot(i);
      const pumping = !!b && active?.run_id === b.run_id;
      const toWeigh = !!b && !pumping && b.weight_g == null;
      const frac = pumping && active?.duration_s ? 1 - (remainingS ?? 0) / active.duration_s : b?.weight_g != null ? 1 : 0;
      return {
        i,
        b,
        pumping,
        toWeigh,
        weighed: b?.weight_g != null,
        frac: Math.max(0, Math.min(1, frac)),
        flow: burstFlow(b),
      };
    }),
  );
  // The measurement being dealt with now: the first one not weighed yet
  // (pumping, to weigh, or next to start). Done ones step back.
  const curIdx = $derived(cards.findIndex((c) => !c.weighed));
  // The one whose detail is open: picked by a click, else the current one.
  let picked = $state(null);
  const selIdx = $derived(picked ?? defaultMeasurement(cards));
  const headline = $derived.by(() => {
    if (curIdx < 0) return { t: 'All 3 measurements weighed', s: 'See the result.' };
    const c = cards[curIdx];
    const n = `Measurement ${curIdx + 1} of 3`;
    if (c.b?.alarm) return { t: `${n}: needs a look`, s: `${ALARM_TEXT[c.b.alarm]} Redo it.` };
    if (c.pumping) return { t: `${n}: pumping`, s: remainingS != null ? `${dur(remainingS)} left` : '' };
    if (c.toWeigh) return { t: `${n}: weigh it`, s: d.auto ? 'the balance settles, then it is weighed' : 'take the balance reading or type the weight' };
    if (d.auto) return { t: `${n}: starting`, s: 'when the balance is still' };
    return { t: `${n}: ready to start`, s: `${d.target_min} min at ${d.setpoint} ${unit}` };
  });

  // How the bursts agree so far (same formula as the record, on what is in).
  const liveCv = $derived.by(() => {
    const f = cards.map((c) => c.flow).filter((x) => x != null);
    if (f.length < 2 || complete) return null;
    const m = f.reduce((a, b) => a + b, 0) / f.length;
    const v = f.reduce((a, x) => a + (x - m) ** 2, 0) / f.length;
    return m > 0 ? (100 * Math.sqrt(v)) / m : null;
  });

  // Result gauges: where a value sits between two bounds, in %.
  const pos = (v, lo, hi) => Math.max(0, Math.min(100, ((v - lo) / (hi - lo)) * 100));
  const c0Lo = $derived(C0_MIN - (C0_MAX - C0_MIN) * 0.25);
  const c0Hi = $derived(C0_MAX + (C0_MAX - C0_MIN) * 0.25);

  let histOpen = $state(false);
  function newTube() {
    saved = null;
    step = 1;
  }
  // The session as it stands (or the calibration just recorded).
  const sum = $derived(saved ?? d);
</script>

<div class="cal">
  <header class="cal-head">
    <div>
      <h2>Tubing calibration</h2>
      <p class="sub">
        The tube's real flow, from three weighed measurements.<Hint label="How the calibration works">
          Weighed by the balance under the feed bottle, or manually. Calibrate the tube before
          autoclaving it.
        </Hint>
      </p>
    </div>
    <span class="bal">
      {#if scaleW != null}
        <span class="dot ok"></span><span>Balance <span class="mono">{num(scaleW, 1)} g</span>, {scaleStable ? 'stable' : 'settling'}</span>
      {:else}
        <span class="dot"></span>Balance not connected
      {/if}
    </span>
  </header>

  {#if !loaded}
    <p class="muted">Loading…</p>
  {:else}
    <!-- The real order of the job; a done step can be reopened. -->
    <Steps label="Calibration steps" current={step} onpick={(n) => (step = n)}
      steps={stepDefs.map((s) => ({ ...s, disabled: !s.reach || (busy && s.n !== 3) }))} />

    <div class="cal-body">
      <section class="panel main-panel">
        {#if step === 1}
          <div class="p-title">
            <h3>Which tube?</h3>
          </div>
          <div class="fields four">
            <div class="field" id="cal-brand"><span>Brand</span>
              <!-- not locked by the bursts: it names the tube, it does not change it -->
              <select aria-label="Brand" value={brandCustom ? '__custom' : (d.brand ?? '')} onchange={(e) => pickBrand(e.currentTarget.value)}
                onblur={() => (left.brand = true)} class:missing={left.brand && !(d.brand ?? '').trim() && !brandCustom}>
                <option value="" disabled>Choose…</option>
                {#each brands as b (b)}<option value={b}>{b}</option>{/each}
                <option value="__custom">Custom…</option>
              </select>
              {#if brandCustom}
                <input type="text" bind:this={brandBox} bind:value={d.brand} onchange={persist} placeholder="Type the brand"
                  aria-label="Custom brand" onblur={() => (left.brand = true)} class:missing={left.brand && !(d.brand ?? '').trim()} />
              {/if}
            </div>
            <label class="field" id="cal-lot"><span>Tubing lot</span>
              <input type="text" bind:value={d.tubing_lot_id} disabled={locked} onchange={persist} placeholder="e.g. LOT-2409" onblur={() => (left.lot = true)} class:missing={left.lot && !d.tubing_lot_id.trim()} />
            </label>
            <label class="field" id="cal-ref"><span>Ref</span>
              <!-- not locked by the bursts: it names the tube, it does not change it -->
              <input type="text" bind:value={d.product_ref} onchange={persist} placeholder="e.g. 06409-14" onblur={() => (left.ref = true)} class:missing={left.ref && !(d.product_ref ?? '').trim()} />
            </label>
            <label class="field"><span>Internal ref (optional)</span>
              <input type="text" bind:value={d.internal_ref} disabled={locked} onchange={persist} />
            </label>
          </div>

          <!-- Diameters: the unit they are typed in, the two of them beside the
               section drawn to scale, the usual sizes in that same unit. -->
          <section class="dia" aria-label="Diameters">
            <div class="dia-head">
              <h4>Diameters</h4>
              <div class="seg small" role="radiogroup" aria-label="Diameters in">
                <button role="radio" aria-checked={unitNow === 'mm'} class:on={unitNow === 'mm'} disabled={locked} onclick={() => setDiaUnit('mm')}>mm</button>
                <button role="radio" aria-checked={unitNow === 'in'} class:on={unitNow === 'in'} disabled={locked} onclick={() => setDiaUnit('in')}>inches</button>
              </div>
            </div>
            <div class="dia-row">
              {#each ['inner', 'outer'] as w (w)}
                {@const mm = Number(d[`${w}_diameter_mm`])}
                <label class="field" id="cal-{w}"><span>{w === 'inner' ? 'Inner' : 'Outer'} Ø, {unitNow === 'in' ? 'inches' : 'mm'}</span>
                  {#if unitNow === 'in'}
                    <input type="text" inputmode="decimal" autocomplete="off" placeholder="e.g. 0.063 or 1/16" value={inchText[w]}
                      disabled={locked} oninput={(e) => typeInches(w, e.currentTarget.value)} onchange={persist} />
                  {:else}
                    <input type="number" step="0.1" min="0" bind:value={d[`${w}_diameter_mm`]} disabled={locked} onchange={persist} />
                  {/if}
                  <span class="conv mono">{mm > 0 ? (unitNow === 'in' ? `= ${num(mm, 2)} mm` : `= ${inches(mm)}`) : ''}</span>
                </label>
              {/each}
              <figure class="tube-fig">
                <svg width="76" height="76" viewBox="0 0 120 120" role="img" aria-label="Tube section to scale">
                  <!-- the diameters alone draw it, lot or not -->
                  {#if tubeSvg.wall != null}
                    <circle class="t-out" cx="60" cy="60" r={tubeSvg.ro} />
                    <circle class="t-in" cx="60" cy="60" r={tubeSvg.ri} />
                  {:else}
                    <circle class="t-empty" cx="60" cy="60" r="52" />
                  {/if}
                </svg>
                <figcaption class="k mono">{tubeSvg.wall != null ? `wall ${num(tubeSvg.wall, 2)} mm` : 'to scale'}</figcaption>
              </figure>
            </div>
            {#if !locked}
              <div class="sizes">
                <span class="k">Common</span>
                {#each COMMON_SIZES as sz (sz[0])}
                  {@const p = sizeParts(sz[0] * MM_PER_IN, sz[1] * MM_PER_IN, unitNow)}
                  <button class="chip bore {boreClass(sz[0] * MM_PER_IN, sz[1] * MM_PER_IN)}" onclick={() => useSize(sz)} title={p.sec}>
                    <span class="mono">{p.pri}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </section>
          {#if sameTube && !locked}
            <p class="again">
              This tube was calibrated {stamp(sameTube.created_at)} (#{sameTube.id}). The new calibration will be the one
              New run uses; the earlier one stays under its older calibrations.
            </p>
          {/if}
          {#if diaBad}<p class="warn">The outer Ø must be larger than the inner Ø.</p>{/if}
          {#if locked}<p class="k">Locked after the first measurement.</p>{/if}
          <div class="nav-row">
            {@render abandonLink()}
            <button class="btn-primary" disabled={!tubeOk} onclick={() => (step = 2)}>Next: pump setup →</button>
          </div>
        {:else if step === 2}
          <div class="p-title">
            <h3>Pump setup</h3>
          </div>
          <div class="fields four">
            <div class="field"><span>Pump direction{#if !directionOk && !locked}<span class="choose">choose</span>{/if}</span>
              <div class="seg" class:need={!directionOk && !locked} id="pump-direction" role="group" aria-label="Pump direction">
                <button disabled={locked} class:on={d.direction === 'cw'} onclick={() => { d.direction = 'cw'; d.direction_set = true; persist(); }} title="Clockwise">↻ cw</button>
                <button disabled={locked} class:on={d.direction === 'ccw'} onclick={() => { d.direction = 'ccw'; d.direction_set = true; persist(); }} title="Counter-clockwise">↺ ccw</button>
              </div>
            </div>
            <label class="field" id="cal-setpoint"><span>Setpoint, {unit}{#if d.control_var === 'rpm'}<Hint label="Why rpm">
                An ml/min run is driven in rpm with this tube's ml per rpm: the pump's head and
                tubing table never come into it.
              </Hint>{/if}</span>
              <input type="number" step={setpointStep(app.status?.pump_model, d.control_var)} min="0" bind:value={d.setpoint} disabled={locked} placeholder="e.g. 100"
                class:missing={!(Number(d.setpoint) > 0)} onchange={() => { d.setpoint_set = true; persist(); }} />
            </label>
            <label class="field" id="cal-burst"><span>Each measurement lasts, min<Hint label="About the measurement time">
                The pump stops by itself after this time, or when you stop it. The flow uses the real
                pumping time.
              </Hint></span>
              <input type="number" step="0.5" min="0" bind:value={d.target_min} onchange={persist} />
            </label>
          </div>
          <label class="field narrow" id="cal-density"><span>Liquid density, g/mL<Hint label="About the liquid density">
              Turns grams into mL. Water and dilute media: 1.00.
            </Hint></span>
            <input type="number" step="0.01" min="0" bind:value={d.density_g_per_ml} disabled={locked} onchange={persist} />
          </label>
          {#if burstEstimate && burstEstimate.stepPct > STEP_LIMIT_PCT}
            <p class="warn">
              {d.target_min} min at {d.setpoint} {unit} collects about {num(burstEstimate.g, 0)} g: one balance step
              ({resG} g) is {num(burstEstimate.stepPct, 1)} % of it. Use {burstEstimate.goodMin} min or more for a
              step under {STEP_LIMIT_PCT} %.
            </p>
          {/if}
          <p class="k">Three measurements: {totalMin >= 60 ? `${Math.floor(totalMin / 60)} h ${Math.round(totalMin % 60)} min` : `${num(totalMin, 0)} min`} in all.</p>
          <div class="field" id="cal-weighing">
            <span>Weighing</span>
            <div class="modes">
              <button class="mode" class:on={weighMode === 'auto'} onclick={() => setWeighMode('auto')}>
                <span class="mode-ic"><Icon name="refresh-cw" size={18} /></span>
                <span class="mode-t">
                  <b>Automatic</b>
                  <span class="k" class:bad={!autoAvailable}>{autoAvailable ? 'Bottle on the balance; no clicks.' : 'Balance disconnected: reconnect it.'}</span>
                </span>
              </button>
              <button class="mode" class:on={weighMode === 'manual'} onclick={() => setWeighMode('manual')}>
                <span class="mode-ic"><Icon name="hand" size={18} /></span>
                <span class="mode-t">
                  <b>Manual</b>
                  <span class="k">Start each measurement, enter its weight.</span>
                </span>
              </button>
            </div>
          </div>
          <label class="field narrow"><span>Operator (optional)</span>
            <input type="text" bind:value={d.operator} onchange={persist} />
          </label>
          {#if locked}<p class="k">Locked after the first measurement.</p>{/if}
          <div class="nav-row">
            {@render abandonLink()}
            <span class="acts">
              <button class="btn-ghost" onclick={() => (step = 1)}>← Tube</button>
              <button class="btn-primary" disabled={!formOk} onclick={() => (step = 3)}>Next: measurements →</button>
            </span>
          </div>
        {:else if step === 3}
          <div class="p-title row">
            <div>
              <h3>Three measurements</h3>
              <p class="k">
                {weighMode === 'auto' ? 'Leave the bottle on the balance.' : 'Start each measurement, then weigh it.'}
                Click a measurement to see it, or to redo it.
              </p>
            </div>
            {#if d.bursts.length}
              <button class="ghost-redo" type="button" disabled={burstRunning || working} aria-expanded={confirmAll}
                onclick={() => { confirmAll = !confirmAll; confirmRedo = null; }}
                title={burstRunning ? 'A measurement is pumping' : 'Drop every measurement and pump the three again, same tube and setup'}>
                <Icon name="refresh-cw" size={16} />Redo all 3
              </button>
            {/if}
          </div>
          {#if confirmAll && d.bursts.length}
            <div class="confirm all">
              <p>
                <b>Redo all 3 measurements?</b>
                {#if allSummary.weights.length}{allSummary.weights.map((w) => `${num(w, wDig)} g`).join(', ')} {allSummary.weights.length === 1 ? 'is' : 'are'} dropped.{/if}
                The tube and the pump setup stay; measurement 1 is pumped next.
                Run{allSummary.runs.length === 1 ? '' : 's'} {allSummary.runs.map((r) => `#${r}`).join(', ')} stay in the journal.
              </p>
              <span class="acts">
                <button class="btn-danger small" disabled={working || burstRunning} onclick={redoAll}>Drop them, start over</button>
                <button class="btn-ghost small" onclick={() => (confirmAll = false)}>Keep them</button>
              </span>
            </div>
          {/if}

          {#if autoBlocked && !burstRunning}
            {#key nudges}
            <div class="blocked" class:nudged={nudges > 0} role="alert">
              <span><b>Balance disconnected.</b> {d.auto ? 'Automatic mode waits for it.' : 'Automatic mode needs it to start.'}</span>
              <span class="acts">
                {#if d.auto}<button class="btn-ghost small" onclick={() => setAuto(false)}>Stop automatic</button>{/if}
                <button class="btn-ghost small" onclick={() => { setAuto(false); setWeighMode('manual'); }}>Switch to manual</button>
              </span>
            </div>
            {/key}
          {/if}
          {#if autoElsewhere}
            <p class="live">Running automatically in another window.</p>
          {:else if d.auto}
            <p class="live">Automatic: {autoStep}.</p>
          {/if}
          {#if otherRunActive}
            <p class="warn">
              {active.kind === 'calibration'
                ? `Measurement run #${active.run_id} is still pumping, outside this session.`
                : 'Another run is active: stop it first.'}
              {#if active.kind === 'calibration'}
                <button class="btn-ghost small" disabled={working} onclick={stopOther}>Stop it</button>
              {/if}
            </p>
          {/if}

          <!-- Nothing here can be done while automatic waits for the balance:
               greyed and inert, so the one way out (the banner above) stands out. -->
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div class="zone-wrap" onclick={() => autoBlocked && !burstRunning && nudges++}>
          <div class="zone" class:inert-zone={autoBlocked && !burstRunning} inert={autoBlocked && !burstRunning}>
          <!-- Where we are, in words, before the cards. -->
          <div class="now-line" class:all={curIdx < 0} aria-live="polite">
            <span class="now-t">{headline.t}</span>
            {#if headline.s}<span class="now-s">{headline.s}</span>{/if}
          </div>

          {#if trouble}
            <div class="stall" role="alert">
              <span>
                <b>{TROUBLE[trouble]}</b>
                ({num(collectedG, wDig)} g in {dur(burstElapsedS)}).
                {trouble === 'reversed'
                  ? 'Is the line still on the pump head and in the liquid, the bottle still on the balance?'
                  : trouble === 'slow'
                    ? `About ${num(burstMlMin, 1)} ml/min expected. Is the line clamped in the pump head, not just siphoning?`
                    : 'Is the bottle on the balance, the line on the pump head and in the liquid, the pump drawing?'}
              </span>
              <span class="acts">
                <button class="btn-danger small" disabled={working} onclick={stopStalled}>Stop the measurement</button>
                <button class="btn-ghost small" onclick={keepGoing}>Keep going</button>
              </span>
            </div>
          {/if}
          <!-- Each measurement a card; the one picked opens its detail below. -->
          <div class="cards" role="tablist" aria-label="Measurements">
            {#each cards as c (c.i)}
              <!-- The card picks the measurement; its Redo is a button of its own
                   beside it (a button cannot hold another). -->
              <div class="bcard" class:current={c.i === curIdx} class:done={c.weighed} class:next={!c.weighed && c.i !== curIdx}
                class:alarm={!!c.b?.alarm} class:sel={c.i === selIdx}>
              <button type="button" role="tab" class="bc-pick" id="m-tab-{c.i}" aria-selected={c.i === selIdx} aria-controls="m-detail"
                onclick={() => { picked = c.i; if (confirmRedo !== c.i) confirmRedo = null; }}>
                <span class="bc-head">
                  <b>{c.b?.alarm ? '✗ ' : c.weighed ? '✓ ' : ''}Measurement {c.i + 1}</b>
                  {#if c.b?.alarm}
                    <span class="btag t-alarm" title={ALARM_TEXT[c.b.alarm]}>check</span>
                  {:else}
                    <span class="btag" class:t-ok={c.weighed} class:t-now={c.i === curIdx}>
                      {c.weighed ? 'done' : c.i === curIdx ? 'now' : 'next'}
                    </span>
                  {/if}
                </span>
                <span class="bc-mid">
                  {#if c.pumping}<PumpHead direction={d.direction} frac={0.6} size={38} />{/if}
                  <span>
                    <span class="bc-big mono">
                      {c.weighed ? `${num(c.b.weight_g, wDig)} g` : (c.pumping || c.toWeigh) && collectedG != null ? `${num(collectedG, wDig)} g` : '–'}
                    </span>
                    <span class="k">
                      {#if c.weighed}
                        {c.flow != null ? `${num(c.flow, sig(c.flow, c.b.weight_g))} ml/min` : 'weighed'}{durations[c.b.run_id] ? `, in ${minSec(durations[c.b.run_id])}` : ''}
                      {:else if c.pumping}
                        drawn{remainingS != null ? `, ${dur(remainingS)} left` : ''}
                      {:else if c.b?.alarm}
                        redo it
                      {:else if c.toWeigh}
                        {c.b.start_g != null ? (weighMode === 'auto' ? 'weighed once the balance is still' : 'take the balance reading') : 'weigh the collected feed'}
                      {:else}
                        {d.target_min} min at {d.setpoint} {unit}
                      {/if}
                    </span>
                  </span>
                </span>
                <span class="bar"><span style="width:{c.b?.alarm ? 100 : c.frac * 100}%" class:full={c.weighed} class:bad={!!c.b?.alarm}></span></span>
                <!-- short: the card's Redo shares the line -->
                <span class="see">{c.i === selIdx ? 'Shown below' : 'Details'}</span>
              </button>
              {#if redoable(c.i)}
                <!-- picks the measurement and asks in its detail (a bad one, with
                     an alarm or never weighed, is dropped at once) -->
                <button class="card-redo" type="button" disabled={working} aria-expanded={confirmRedo === c.i}
                  aria-label="Redo measurement {c.i + 1}" title="Drops this measurement and pumps it again: after air, a leak or a lost drop."
                  onclick={() => { picked = c.i; confirmAll = false; if (confirmRedo === c.i) confirmRedo = null; else redoBurst(c.i); }}>
                  <Icon name="refresh-cw" size={14} />Redo
                </button>
              {/if}
              </div>
            {/each}
          </div>

          {#if cards[selIdx]}
            {@const c = cards[selIdx]}
            {@const pr = c.b ? profiles[c.b.run_id] : null}
            <div class="detail" role="tabpanel" id="m-detail" aria-labelledby="m-tab-{c.i}">
              <div class="d-head">
                <div class="d-id">
                  <h4>Measurement {c.i + 1}</h4>
                  {#if c.b}<span class="k mono">{runStarts[c.b.run_id] ? `${stamp(runStarts[c.b.run_id])} · ` : ''}run #{c.b.run_id}</span>{/if}
                </div>
                <span class="d-acts">
                  {#if c.weighed && c.b.start_g == null && !d.auto && editing !== c.i}
                    <!-- a weight typed by hand (no balance) can be corrected; one read off the balance is redone -->
                    <button class="btn-ghost small" disabled={working} onclick={() => startEdit(c.i)}>Edit weight</button>
                  {/if}
                </span>
              </div>

              {#if confirmRedo === c.i && c.b}
                <div class="confirm">
                  <p><b>Redo measurement {c.i + 1}?</b> Its {num(c.b.weight_g, wDig)} g is dropped and measurement {c.i + 1} is pumped next.</p>
                  <span class="acts">
                    <button class="btn-danger small" disabled={working} onclick={() => redoBurst(c.i)}>Drop it, redo</button>
                    <button class="btn-ghost small" onclick={() => (confirmRedo = null)}>Keep it</button>
                  </span>
                </div>
              {/if}

              {#if editing === c.i}
                <span class="acts edit">
                  <input class="edit-w" type="number" step="0.01" min="0" bind:value={editInput} aria-label="Collected weight, g"
                    onkeydown={(e) => { if (e.key === 'Enter') saveEdit(); if (e.key === 'Escape') editing = null; }} /> g
                  <button class="btn-ghost small" disabled={!(Number(editInput) > 0)} onclick={saveEdit}>Save</button>
                  <button class="btn-ghost small" onclick={() => (editing = null)}>Cancel</button>
                </span>
              {/if}

              {#if c.weighed}
                <div class="figs">
                  <div class="fig"><div class="v mono">{num(c.b.weight_g, wDig)}<small>g</small></div><div class="l">collected</div></div>
                  <div class="fig"><div class="v mono">{c.flow != null ? num(c.flow, sig(c.flow, c.b.weight_g)) : '·'}<small>ml/min</small></div><div class="l">flow</div></div>
                  <div class="fig"><div class="v mono">{durations[c.b.run_id] ? minSec(durations[c.b.run_id]) : '·'}</div><div class="l">pumped</div></div>
                  <div class="fig" title="Balance read every second: flow from the slope (first {SKIP_START_S} s and last {SKIP_END_S} s left out), and its change from the first half to the second.">
                    <div class="v mono">{pr ? num(pr.gPerMin, 3) : '·'}<small>g/min</small></div>
                    <div class="l">slope{pr?.driftPct != null ? `, ${pr.driftPct >= 0 ? '+' : '−'}${num(Math.abs(pr.driftPct), 1)} % 2nd half` : ''}</div>
                  </div>
                </div>
                {#if pr}
                  <div>
                    <div class="ctitle">
                      <b>On the balance, every second</b>
                      <span class="legend"><span><i class="sw w"></i>collected, g</span><span><i class="sw fit"></i>slope</span><span><i class="sw skip"></i>left out</span></span>
                    </div>
                    <MeasurementChart profile={pr} label="Weight collected during measurement {c.i + 1}" />
                  </div>
                {/if}
              {:else if c.b?.alarm}
                <p class="warn">{ALARM_TEXT[c.b.alarm]} Redo it.</p>
              {:else if c.pumping}
                <p class="k">Pumping{remainingS != null ? `, ${dur(remainingS)} left` : ''}{collectedG != null ? `, ${num(collectedG, wDig)} g so far` : ''}.</p>
              {:else if c.toWeigh}
                <p class="k">Pumped: {c.b.start_g != null ? (weighMode === 'auto' ? 'weighed once the balance is still.' : 'take the balance reading below.') : 'weigh the collected feed below.'}</p>
              {:else}
                <p class="k">
                  Not pumped yet: {d.target_min} min at {d.setpoint} {unit}.
                  {c.i === freeSlot ? 'It is pumped next.' : `Measurement ${(freeSlot ?? 0) + 1} comes first.`}
                </p>
              {/if}
            </div>
          {/if}

          <!-- a burst with an alarm is pumped again, not weighed -->
          {#if needsWeight && !d.auto && !current.alarm}
            <div class="weigh-row">
              {#if collectedG != null}
                <button class="btn-primary" disabled={!scaleStable || !avgW || !(weighG > 0)} onclick={saveBalanceReading}>
                  Take balance reading: {num(weighG, 2)} g
                </button>
                <span class="live" class:settling={!scaleStable}>
                  {scaleStable && avgW
                    ? `mean of ${avgW.n} stable reading${avgW.n === 1 ? '' : 's'} (${num(avgW.g, 2)} g), ${num(current.start_g, 2)} g before`
                    : `settling…, ${num(scaleW, 2)} g now, ${num(current.start_g, 2)} g before`}
                </span>
              {/if}
              <!-- With the balance reading, the weight is the balance's alone: a
                   wrong one is pumped again, never typed over. Typed only when
                   there is no balance reading to take. -->
              {#if collectedG == null}
                <label class="field weigh"><span>{current?.start_g != null ? 'Balance not reading: collected weight, g' : 'Collected weight, g'}</span>
                  <input type="number" step="0.01" min="0" bind:value={weightInput} onkeydown={(e) => e.key === 'Enter' && saveWeight()} />
                </label>
                <button class="btn-primary" disabled={!(Number(weightInput) > 0)} onclick={saveWeight}>Save typed weight</button>
              {/if}
            </div>
          {:else if !burstRunning && !d.auto && d.bursts.length < 3 && scaleW != null && !needsWeight}
            <p class="k">Start weight {num(scaleW, 2)} g{scaleStable ? '' : ', settling'}.</p>
          {/if}

          {#if liveCv != null}
            <p class="spread"><span class="dot" class:ok={!cvBad(liveCv)} class:bad-dot={cvBad(liveCv)}></span>
              Spread so far <b class="mono">{num(liveCv, 2)} %</b> (limit {CV_WARN_PCT} %).
            </p>
          {/if}
          </div>
          </div>

          <div class="nav-row">
            {@render abandonLink()}
            <span class="acts">
              <button class="btn-ghost" disabled={busy} onclick={() => (step = 2)}>← Pump setup</button>
              <!-- The next thing to do sits where the eye ends: run or stop a
                   burst, and the result once all three are weighed. -->
              {#if complete}
                <button class="btn-primary" onclick={() => (step = 4)}>See the result →</button>
              {:else}
                {#if autoElsewhere}
                  <button class="btn-ghost" onclick={() => setAuto(true)}>Take over here</button>
                  <button class="btn-danger" onclick={() => setAuto(false)}>Stop automatic</button>
                {:else if d.auto}
                  <button class="btn-danger" onclick={() => setAuto(false)}>Stop automatic</button>
                {:else if burstRunning}
                  <button class="btn-danger" disabled={working} onclick={restartBurst}
                    title="Stops the pump and pumps this measurement again from the start: a part measurement is never weighed">↻ Restart this measurement</button>
                {:else if alarmed}
                  <span class="held">Measurement {(alarmed.slot ?? 0) + 1} needs a look: redo it first.</span>
                {:else if !needsWeight && d.bursts.length < 3 && !autoBlocked}
                  {#if weighMode === 'auto' && canAuto}
                    <button class="btn-primary" disabled={!formOk || working} onclick={() => setAuto(true)}>
                      Run {d.bursts.length ? 'the remaining measurements' : 'all 3 measurements'}
                    </button>
                  {:else}
                    <button class="btn-primary" disabled={!canStartBurst} onclick={startBurst}>
                      {working ? 'Starting…' : `Start measurement ${(freeSlot ?? 0) + 1}/3`}
                    </button>
                  {/if}
                {/if}
              {/if}
            </span>
          </div>
        {:else}
          <div class="p-title">
            <h3>{sum.tubing_lot_id}, {num(sum.inner_diameter_mm, 1)} / {num(sum.outer_diameter_mm, 1)} mm</h3>
            <p class="k">At {sum.setpoint} {unitFor(sum.control_var)}</p>
          </div>
          {#if shown}
            {@const cvar = saved?.control_var ?? d.control_var}
            {@const sp = saved?.setpoint ?? Number(d.setpoint)}
            {@const ws = saved?.weights_g ?? d.bursts.map((b) => b.weight_g)}
            {@const wMin = Math.min(...ws)}
            {@const stepPct = (100 * resG) / wMin}
            <div class="res-top">
              <div>
                <div class="k">Real flow</div>
                <div class="res-big"><span>{num(shown.mean_measured_ml_min, sig(shown.mean_measured_ml_min, wMin))}</span><span class="res-u">ml/min</span></div>
              </div>
              {#if cvar === 'ml_min'}
                <div>
                  <div class="k">c₀, trim start</div>
                  <div class="res-mid mono" class:bad={c0Bad('ml_min', shown.c0)}>×{num(shown.c0, sig(shown.c0, wMin))}</div>
                </div>
              {:else}
                <div>
                  <div class="k">Per rpm</div>
                  <div class="res-mid mono">{num(shown.mean_measured_ml_min / sp, sig(shown.mean_measured_ml_min / sp, wMin))} ml/min</div>
                </div>
              {/if}
            </div>

            <!-- where each quality figure sits against its limit -->
            <div class="gauges">
              <div class="gauge">
                <div class="g-head"><span class="lbl">Spread (CV)</span><span class="mono">{num(shown.cv_pct, stepPct >= 1 ? 1 : 2)} %</span></div>
                <div class="g-track cv"><span class="g-mark" class:bad-mark={cvBad(shown.cv_pct)} style="left:calc({pos(shown.cv_pct, 0, 2 * CV_WARN_PCT)}% - 3px)"></span></div>
                <div class="g-scale mono"><span>0 %</span><span>limit {CV_WARN_PCT} %</span><span>{2 * CV_WARN_PCT} %</span></div>
              </div>
              {#if cvar === 'ml_min'}
                <div class="gauge">
                  <div class="g-head"><span class="lbl">c₀ vs trim limit</span><span class="mono">×{num(shown.c0, sig(shown.c0, wMin))}</span></div>
                  <div class="g-track c0" style="--lo:{pos(C0_MIN, c0Lo, c0Hi)}%; --hi:{pos(C0_MAX, c0Lo, c0Hi)}%">
                    <span class="g-one" style="left:{pos(1, c0Lo, c0Hi)}%"></span>
                    <span class="g-mark" class:bad-mark={c0Bad('ml_min', shown.c0)} style="left:calc({pos(shown.c0, c0Lo, c0Hi)}% - 3px)"></span>
                  </div>
                  <!-- each label under the point it names on the track -->
                  <div class="g-scale at mono">
                    <span style="left:{pos(C0_MIN, c0Lo, c0Hi)}%">×{num(C0_MIN, 2)}</span>
                    <span style="left:{pos(1, c0Lo, c0Hi)}%">×1.00</span>
                    <span style="left:{pos(C0_MAX, c0Lo, c0Hi)}%">×{num(C0_MAX, 2)}</span>
                  </div>
                </div>
              {/if}
            </div>

            <div class="flows">
              <div class="fl-head">
                <span class="lbl">Each measurement</span>
                {#if !saved}
                  <!-- back to the measurements, "drop them all?" already asked;
                       one measurement is redone from its card there -->
                  <button class="ghost-redo" type="button" onclick={() => { confirmAll = true; confirmRedo = null; step = 3; }}>
                    <Icon name="refresh-cw" size={16} />Redo all 3
                  </button>
                {/if}
              </div>
              {#each shown.measured_ml_min as m, i}
                {@const mean = shown.mean_measured_ml_min}
                <div class="f-row">
                  <span>Measurement {i + 1}</span>
                  <span class="f-track"><span style="width:{pos(m, mean * 0.95, mean * 1.05)}%"></span></span>
                  <span class="mono">{num(m, sig(m, ws[i]))} ml/min</span>
                </div>
              {/each}
            </div>

            {#if stepPct >= STEP_LIMIT_PCT * 2}
              <p class="warn">
                One balance step ({resG} g) is {num(stepPct, 1)} % of a measurement: the spread and the flow are only
                known to about that. Longer measurements make it finer.
              </p>
            {/if}
            {#if cvBad(shown.cv_pct)}
              <p class="warn">Measurements differ by over {CV_WARN_PCT} %: check for air, a leak or a missed drop, then redo.</p>
            {/if}
            {#if c0Bad(cvar, shown.c0)}
              <p class="warn">c₀ outside ×{C0_MIN} to ×{C0_MAX}: the trim starts clamped there.</p>
            {/if}

            {#if !saved}
              <div class="rec-row">
                <label class="field grow"><span>Note (optional)</span>
                  <input type="text" bind:value={d.note} onchange={persist} placeholder="e.g. new roll, first use" />
                </label>
                <button class="btn-primary" disabled={working || !namesOk} onclick={record}
                  title={namesOk ? undefined : "Enter the tube's brand and Ref first, at the Tube step"}>Record calibration</button>
              </div>
              <div class="nav-row">
                {@render abandonLink()}
              </div>
            {:else}
              <div class="recorded">
                <span><b>Recorded as #{saved.id}.</b> Pick it in New run with the balance trim.</span>
                <span class="acts">
                  <button class="btn-ghost" onclick={newTube}>Calibrate another tube</button>
                  <button class="btn-primary" onclick={() => (app.tab = 'new')}>Start a run with it</button>
                </span>
              </div>
            {/if}
          {:else}
            <p class="muted">Loading measurement times…</p>
          {/if}
        {/if}

        {#if err}
          <ErrorText message={err.message} hint={err.hint} />
        {/if}
      </section>

      <!-- The session, always in view -->
      <aside class="panel side">
        <h3>{saved ? 'Just recorded' : 'This session'}</h3>
        <dl class="sum">
          <dt>Tube</dt>
          <dd>{sum.tubing_lot_id?.trim() ? `${sum.tubing_lot_id}, ${num(sum.inner_diameter_mm, 1)} / ${num(sum.outer_diameter_mm, 1)} mm` : 'not set yet'}</dd>
          <dt>Setpoint</dt><dd class="mono">{Number(sum.setpoint) > 0 ? `${sum.setpoint} ${unitFor(sum.control_var)}` : 'to set'}{saved ? '' : `, ${d.direction ?? 'direction to choose'}`}</dd>
          {#if !saved}<dt>Measurements</dt><dd class="mono">{d.target_min} min each</dd>{/if}
          <dt>Density</dt><dd class="mono">{num(sum.density_g_per_ml, 2)} g/mL</dd>
          {#if !saved}
            <dt>Weighing</dt><dd>{weighMode === 'auto' ? 'automatic' : 'manual'}{autoBlocked ? ', no balance' : ''}</dd>
            <dt>Progress</dt><dd class="mono">{d.bursts.filter((b) => b.weight_g != null).length} of 3 weighed</dd>
          {/if}
        </dl>
        {#if !saved}
          <div class="side-foot">
            <span class="saved-note"><span class="tick" aria-hidden="true">✓</span>
              Saved as you go. Leaving this page asks first, then clears it.</span>
          </div>
        {/if}
      </aside>
    </div>

    <!-- History, out of the way until needed -->
    <section class="panel hist-panel">
      <div class="hist-head">
        <button class="hist-toggle" aria-expanded={histOpen} onclick={() => (histOpen = !histOpen)}>
          <span class="chev" class:open={histOpen}>›</span>
          Recorded tubes
          <span class="k">{history.length - archivedCount} in use{archivedCount ? `, ${archivedCount} archived` : ''}</span>
        </button>
        <span class="acts">
          {#if histOpen && archivedCount}
            <label class="toggle"><input type="checkbox" bind:checked={showArchived} /> Show archived</label>
          {/if}
          {#if history.length}<button class="btn-ghost" onclick={exportCsv}>Export CSV</button>{/if}
          <Hint label="About archiving">
            Archived tubes leave New run but stay on record. Restore brings them back.
          </Hint>
        </span>
      </div>
      {#if histOpen}
        {#if listed.length}
          <div class="hist-wrap">
            <table class="hist mono">
              <thead>
                <tr><th>#</th><th>date</th><th>brand</th><th>lot</th><th>ref</th><th>internal ref</th><th>size</th><th>setpoint</th><th>mean ml/min</th><th>CV %</th><th>pump</th><th>c₀</th><th></th></tr>
              </thead>
              <tbody>
                {#each listed as c (c.id)}
                  {@const sz = sizeParts(c.inner_diameter_mm, c.outer_diameter_mm, app.tubeSizes)}
                  <tr class:archived={c.archived_at} class={boreClass(c.inner_diameter_mm, c.outer_diameter_mm)}>
                    <td>{c.id}</td>
                    <td>{stamp(c.created_at)}</td>
                    <td>{c.brand ?? ''}</td>
                    <td>{c.tubing_lot_id}</td>
                    <td>{c.product_ref ?? ''}</td>
                    <td>{c.internal_ref ?? ''}</td>
                    <td><span class="bore-tag">{sz.pri}<span class="sec">{sz.sec}</span></span></td>
                    <td>{num(c.setpoint, digitsFor(c.control_var))} {unitFor(c.control_var)}</td>
                    <td>{num(c.mean_measured_ml_min, sig(c.mean_measured_ml_min, Math.min(...(c.weights_g ?? []))))}</td>
                    <td class:bad={cvBad(c.cv_pct)}>{num(c.cv_pct, 2)}</td>
                    <td class="pm">{modelOf(c.pump_model).label}</td>
                    <td class:bad={c0Bad(c.control_var, c.c0)}>{c.control_var === 'ml_min' ? num(c.c0, sig(c.c0, Math.min(...(c.weights_g ?? [])))) : '·'}</td>
                    <td class="row-action">
                      {#if c.archived_at}
                        <span class="tag" title="Archived {stamp(c.archived_at)}">archived</span>
                        <button class="btn-ghost small" disabled={archiving === c.id} onclick={() => setArchived(c, false)}>Restore</button>
                      {:else}
                        <button class="btn-ghost small" disabled={archiving === c.id} onclick={() => setArchived(c, true)}>Archive</button>
                      {/if}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else if history.length}
          <p class="muted">All calibrations are archived.</p>
        {:else}
          <p class="muted">None yet.</p>
        {/if}
      {/if}
    </section>
  {/if}
</div>

{#snippet abandonLink()}
  {#if abandonable}
    <button class="abandon" disabled={working} onclick={() => (confirmDiscard = true)}>Abandon calibration</button>
  {:else}
    <span></span>
  {/if}
{/snippet}

{#if confirmDiscard}
  {@const pumped = d.bursts.length}
  <button class="backdrop" aria-label="Cancel" onclick={() => !working && (confirmDiscard = false)}></button>
  <div class="layer">
    <div class="modal card confirm" role="alertdialog" aria-modal="true" aria-labelledby="ab-title">
      <h2 id="ab-title">Abandon this calibration?</h2>
      <ul class="lede">
        {#if burstRunning}<li><b>The measurement in progress stops the pump.</b></li>{/if}
        <li>
          {#if pumped}The tube, the setpoint and the {pumped} burst{pumped === 1 ? '' : 's'} so far are cleared.
          {:else}The tube details typed so far are cleared.{/if}
        </li>
        <li>You start again at step 1, Tube.</li>
        <li>Nothing is recorded as a calibration.{pumped ? ' The measurements already pumped stay in the journal.' : ''}</li>
      </ul>
      <div class="modal-foot">
        <button class="btn-ghost" disabled={working} onclick={() => (confirmDiscard = false)}>Keep calibrating</button>
        <button class="btn-danger" disabled={working}
          onclick={async () => { await discard(); if (!err) step = 1; }}>{burstRunning ? 'Stop the pump and abandon' : 'Abandon'}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .cal { display: flex; flex-direction: column; gap: var(--s-4); }
  .cal-head { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: flex-end; gap: var(--s-3); }
  .cal-head h2 { font-size: 22px; }
  .sub { margin: var(--s-1) 0 0; color: var(--muted); max-width: 70ch; }
  .bal { display: inline-flex; align-items: center; gap: var(--s-2); font-size: 13px; color: var(--muted); }
  .bal .mono { color: var(--ink); }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--muted); flex: none; display: inline-block; }
  .dot.ok { background: var(--green-500); }
  .dot.bad-dot { background: var(--danger); }
  .k { font-size: 12.5px; color: var(--muted); margin: 0; }
  .lbl { font-size: 12.5px; color: var(--muted); font-weight: 500; }
  .muted { color: var(--muted); }
  .warn { font-size: 13px; color: var(--danger-ink); margin: 0; }
  .k.bad { color: var(--danger-ink); }
  .stall {
    display: flex; flex-wrap: wrap; justify-content: space-between; align-items: center; gap: var(--s-2) var(--s-4);
    padding: var(--s-3) var(--s-4); border-radius: var(--radius-ctl); font-size: 13.5px;
    background: var(--danger-bg); color: var(--danger-ink);
    border: 1px solid color-mix(in srgb, var(--danger) 30%, var(--surface));
  }
  .stall b { color: var(--danger-ink); }
  .blocked {
    display: flex; flex-wrap: wrap; justify-content: space-between; align-items: center; gap: var(--s-2) var(--s-4);
    padding: var(--s-3) var(--s-4); border-radius: var(--radius-ctl);
    background: var(--danger-bg); color: var(--danger-ink); font-size: 13.5px;
    border: 1px solid color-mix(in srgb, var(--danger) 30%, var(--surface));
    /* in with a slide and a short shake, then a slow glow while it lasts */
    animation: blk-in 0.45s ease-out, blk-glow 2.2s ease-in-out 0.6s infinite;
  }
  .blocked.nudged { animation: blk-shake 0.4s ease-in-out, blk-glow 2.2s ease-in-out 0.6s infinite; }
  @keyframes blk-in {
    0% { opacity: 0; transform: translateY(-6px); }
    40% { opacity: 1; transform: translateY(0); }
    55% { transform: translateX(-6px); }
    70% { transform: translateX(5px); }
    85% { transform: translateX(-3px); }
    100% { transform: none; }
  }
  @keyframes blk-shake {
    20% { transform: translateX(-7px); }
    40% { transform: translateX(6px); }
    60% { transform: translateX(-4px); }
    80% { transform: translateX(2px); }
  }
  @keyframes blk-glow {
    0%, 100% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--danger) 0%, transparent); }
    50% { box-shadow: 0 0 0 5px color-mix(in srgb, var(--danger) 18%, transparent); }
  }
  /* a real box: clicks on the inert zone fall through to it */
  .zone-wrap { display: flex; flex-direction: column; }
  .bad { color: var(--danger-ink); }
  .panel {
    background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius);
    padding: var(--s-5); min-width: 0;
  }


  .cal-body { display: flex; flex-wrap: wrap; gap: var(--s-4); align-items: flex-start; }
  .main-panel { flex: 999 1 560px; display: flex; flex-direction: column; gap: var(--s-5); padding: var(--s-6); }
  .side { flex: 1 1 250px; display: flex; flex-direction: column; gap: var(--s-3); }
  .p-title h3, .side h3 { margin: 0; font-size: 17px; font-weight: 600; letter-spacing: 0; }
  .side h3 { font-size: 15px; }
  .p-title.row { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: flex-end; gap: var(--s-3); }
  .acts { display: inline-flex; flex-wrap: wrap; gap: var(--s-2); align-items: center; }
  .nav-row { display: flex; justify-content: space-between; align-items: center; gap: var(--s-3); flex-wrap: wrap; }

  .fields { display: grid; gap: var(--s-4); }
  .fields.two { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .fields.four { grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); }
  .field.narrow { max-width: 280px; }
  .field.grow { flex: 1 1 260px; }
  .chip {
    padding: 5px 11px; border-radius: 999px; border: 1px solid var(--line);
    background: var(--surface); color: var(--ink); font-size: 12.5px;
  }
  .chip:not(:disabled):hover { border-color: var(--teal-400); }
  /* a tube or a size, in its bore's pastel */
  .chip.bore { background: var(--c-bg); border-color: var(--c-line); color: var(--c-ink); }
  .chip.bore:not(:disabled):hover { border-color: var(--c-ink); }
  input.missing, select.missing { border-color: var(--measured); }
  .conv { min-height: 1.3em; font-size: 12px; color: var(--teal-700); }
  .dia { display: flex; flex-direction: column; gap: var(--s-3); padding-top: var(--s-4); border-top: 1px solid var(--line-soft); }
  .dia-head { display: flex; justify-content: space-between; align-items: center; gap: var(--s-3); }
  .dia-head h4 { margin: 0; font-size: 14px; }
  .seg.small button { padding: 4px 12px; font-size: 13px; }
  .dia-row { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) auto; gap: var(--s-4); align-items: start; }
  @media (max-width: 560px) { .dia-row { grid-template-columns: 1fr 1fr; } .dia-row .tube-fig { display: none; } }
  .sizes { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
  .sizes .k { margin-right: var(--s-1); font-size: 12px; }
  .sizes .chip { padding: 2px 9px; font-size: 12px; }
  .again { margin: 0; padding: var(--s-2) var(--s-3); border-radius: var(--radius-ctl); font-size: 13px; background: var(--surface-sunken); color: var(--ink); }
  .sec { color: var(--muted); font-size: 0.9em; margin-left: 6px; }

  .tube-fig { margin: 0; display: flex; flex-direction: column; align-items: center; gap: 2px; }
  .tube-fig figcaption { font-size: 11px; }
  .t-out { fill: color-mix(in srgb, var(--teal-400) 22%, var(--surface)); stroke: var(--teal-700); stroke-width: 2; }
  .t-in { fill: var(--surface); stroke: var(--teal-700); stroke-width: 1.5; }
  .t-empty { fill: none; stroke: var(--line); stroke-width: 2; stroke-dasharray: 5 5; }

  .modes { display: grid; grid-template-columns: repeat(auto-fit, minmax(230px, 1fr)); gap: var(--s-3); }
  .mode {
    display: flex; align-items: center; gap: var(--s-3); text-align: left;
    padding: var(--s-3) var(--s-4); border-radius: var(--radius-ctl);
    border: 1px solid var(--line); background: var(--surface); color: var(--ink);
  }
  .mode-t { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .mode-ic {
    flex: none; width: 34px; height: 34px; border-radius: 50%;
    display: grid; place-items: center;
    background: color-mix(in srgb, var(--teal-700) 10%, var(--surface)); color: var(--teal-700);
  }
  .mode.on .mode-ic { background: var(--teal-700); color: var(--on-primary); }
  .mode.on { border: 2px solid var(--teal-700); background: color-mix(in srgb, var(--teal-700) 5%, var(--surface)); }

  /* bursts */
  .live { font-size: 13px; color: var(--teal-700); margin: 0; }
  .live.settling { color: var(--muted); }
  .cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr)); gap: var(--s-3); }
  .zone { display: flex; flex-direction: column; gap: var(--s-5); }
  .inert-zone { opacity: 0.4; filter: grayscale(1); user-select: none; }
  .now-line {
    display: flex; flex-wrap: wrap; align-items: baseline; gap: var(--s-1) var(--s-3);
    padding: var(--s-3) var(--s-4); border-radius: var(--radius-ctl);
    background: color-mix(in srgb, var(--teal-700) 9%, var(--surface));
    border-left: 4px solid var(--teal-700);
  }
  .now-line.all { background: color-mix(in srgb, var(--lime-300) 42%, var(--surface)); border-left-color: var(--green-500); }
  .now-t { font-size: 19px; font-weight: 600; }
  .now-s { color: var(--muted); }
  .bcard {
    position: relative; display: flex; flex-direction: column;
    padding: var(--s-4); border-radius: var(--radius-ctl); border: 1px solid var(--line); background: var(--surface);
  }
  .bcard:hover { border-color: var(--teal-400); }
  /* the whole card picks; its content is the pick button */
  .bc-pick {
    display: flex; flex-direction: column; gap: var(--s-2); width: 100%; padding: 0; margin: 0;
    font: inherit; color: inherit; text-align: left; background: none; border: none; cursor: pointer; border-radius: 4px;
  }
  .bc-pick:focus-visible { outline: 2px solid var(--teal-600); outline-offset: 6px; }
  /* the card's own Redo, in the corner of the "See" line */
  .card-redo {
    position: absolute; right: var(--s-4); bottom: var(--s-3);
    display: inline-flex; align-items: center; gap: 5px; padding: 3px 10px; border-radius: var(--radius-ctl);
    border: 1.5px solid var(--teal-700); background: var(--surface); color: var(--teal-700);
    font: inherit; font-size: 12.5px; font-weight: 600; cursor: pointer; white-space: nowrap;
  }
  .card-redo:not(:disabled):hover { background: var(--teal-700); color: var(--on-primary); }
  .card-redo:focus-visible { outline: 2px solid var(--teal-600); outline-offset: 2px; }
  .card-redo:disabled { opacity: 0.5; cursor: default; }
  .card-redo[aria-expanded="true"] { background: var(--surface-sunken); border-color: var(--line); color: var(--muted); }
  .bc-pick .k { display: block; }
  /* picked: the card the detail below is about, joined to it by a notch */
  .bcard.sel { outline: 2px solid var(--ink); outline-offset: 2px; }
  .bcard.sel::after {
    content: ""; position: absolute; left: 50%; bottom: -15px; width: 14px; height: 14px;
    transform: translateX(-50%) rotate(45deg);
    background: var(--surface); border-left: 1px solid var(--line); border-top: 1px solid var(--line);
  }
  .see { min-height: 28px; display: flex; align-items: center; font-size: 12px; font-weight: 600; color: var(--teal-700); }
  .bcard.sel .see { color: var(--ink); }
  /* now: the strongest thing on the screen */
  .bcard.current {
    border: 2px solid var(--teal-700);
    box-shadow: 0 6px 18px color-mix(in srgb, var(--teal-700) 18%, transparent);
  }
  /* done: kept readable, stepped back */
  .bcard.done { background: var(--surface-sunken); border-color: transparent; }
  .bcard.done .bc-big { font-size: 19px; }
  .bcard.done .bc-head b { color: var(--green-600); }
  /* coming: an outline only */
  .bcard.next { border-style: dashed; background: transparent; }
  .bcard.next .bc-head b, .bcard.next .bc-big { color: var(--muted); }
  .bc-head { display: flex; justify-content: space-between; align-items: center; }
  .btag { font-size: 11.5px; font-weight: 600; padding: 2px 9px; border-radius: 999px; background: var(--surface-sunken); color: var(--muted); }
  .btag.t-ok { background: color-mix(in srgb, var(--green-500) 16%, var(--surface)); color: var(--green-600); }
  .btag.t-now { background: var(--teal-700); color: var(--on-primary); text-transform: uppercase; letter-spacing: 0.06em; }
  .held { font-size: 13px; font-weight: 600; color: var(--danger-ink); }
  .btag.t-alarm { background: var(--danger-bg); color: var(--danger-ink); text-transform: uppercase; letter-spacing: 0.06em; }
  .bc-mid { display: flex; align-items: center; gap: var(--s-3); min-height: 52px; }
  .bc-big { display: block; font-size: 24px; font-weight: 500; line-height: 1.15; }
  .bar { display: block; height: 6px; border-radius: 999px; background: var(--surface-sunken); overflow: hidden; }
  .bar span { display: block; height: 100%; border-radius: 999px; background: var(--teal-700); transition: width 0.5s linear; }
  .bar span.full { background: var(--green-500); }
  .bar span.bad { background: var(--danger); }
  /* a measurement with an alarm: red cross, red bar, red ground, until redone */
  .bcard.alarm { background: var(--danger-bg); border: 1px solid color-mix(in srgb, var(--danger) 45%, var(--surface)); }
  .bcard.alarm .bc-head b { color: var(--danger-ink); }
  /* redo all 3: quieter than one redo, it undoes more */
  .ghost-redo {
    display: inline-flex; align-items: center; gap: 6px; padding: 6px 12px; border-radius: var(--radius-ctl);
    border: 1px solid var(--line); background: var(--surface); color: var(--ink);
    font: inherit; font-size: 13px; font-weight: 500; cursor: pointer; white-space: nowrap;
  }
  .ghost-redo:not(:disabled):hover { border-color: var(--teal-700); color: var(--teal-700); }
  .ghost-redo:focus-visible { outline: 2px solid var(--teal-600); outline-offset: 2px; }
  .ghost-redo:disabled { opacity: 0.5; cursor: default; }
  .ghost-redo[aria-expanded="true"] { background: var(--surface-sunken); color: var(--muted); }
  .confirm.all { display: flex; flex-direction: column; gap: var(--s-2); padding: var(--s-3) var(--s-4); border-radius: var(--radius-ctl); background: var(--surface-sunken); }
  .confirm.all p { margin: 0; font-size: 13px; }
  .confirm.all .acts { display: flex; flex-wrap: wrap; gap: var(--s-2); }

  /* the picked measurement */
  .detail { border: 1px solid var(--line); border-radius: var(--radius); padding: var(--s-5); display: flex; flex-direction: column; gap: var(--s-4); }
  .d-head { display: flex; justify-content: space-between; align-items: center; gap: var(--s-3); flex-wrap: wrap; }
  .d-id { display: flex; flex-direction: column; }
  .d-id h4 { margin: 0; font-size: 16px; }
  .d-id .k { font-size: 12.5px; }
  .d-acts { display: flex; align-items: center; gap: var(--s-2); flex-wrap: wrap; }
  .detail .confirm { display: flex; flex-direction: column; gap: var(--s-2); padding: var(--s-3) var(--s-4); border-radius: var(--radius-ctl); background: var(--surface-sunken); }
  .detail .confirm p { margin: 0; font-size: 13px; }
  .detail .acts { display: flex; flex-wrap: wrap; align-items: center; gap: var(--s-2); }
  .detail > p { margin: 0; }
  .figs { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); border: 1px solid var(--line); border-radius: var(--radius-ctl); }
  .fig { padding: var(--s-2) var(--s-3); min-width: 0; }
  .fig + .fig { border-left: 1px solid var(--line); }
  .fig .v { font-size: 18px; font-weight: 600; line-height: 1.25; }
  .fig .v small { font-size: 12px; font-weight: 500; color: var(--muted); margin-left: 2px; }
  .fig .l { font-size: 12px; color: var(--muted); }
  .ctitle { display: flex; justify-content: space-between; align-items: baseline; gap: var(--s-3); flex-wrap: wrap; font-size: 12px; color: var(--muted); margin-bottom: var(--s-1); }
  .ctitle b { color: var(--ink); font-size: 13px; }
  .legend { display: flex; gap: var(--s-4); flex-wrap: wrap; }
  .legend span { display: inline-flex; align-items: center; gap: 6px; white-space: nowrap; }
  .sw { display: inline-block; width: 16px; height: 3px; border-radius: 2px; }
  .sw.w { background: var(--green-500); }
  .sw.fit { background: repeating-linear-gradient(90deg, var(--ink) 0 4px, transparent 4px 7px); opacity: 0.6; }
  .sw.skip { width: 12px; height: 12px; border-radius: 3px; background: var(--surface-sunken); border: 1px solid var(--line); }
  @media (max-width: 720px) {
    .bcard.sel::after { display: none; }
    .figs { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .fig:nth-child(3) { border-left: none; }
    .fig:nth-child(n + 3) { border-top: 1px solid var(--line); }
  }
  .edit { display: inline-flex; align-items: center; gap: var(--s-1); }
  .edit-w { width: 100px; }
  .btn-ghost.small, .btn-danger.small { padding: 2px var(--s-2); font-size: 12px; }
  .weigh-row { display: flex; flex-wrap: wrap; gap: var(--s-3); align-items: flex-end; }
  .weigh { width: 180px; }
  .spread { display: flex; align-items: center; gap: var(--s-2); font-size: 13px; margin: 0; }

  /* result */
  .res-top { display: flex; flex-wrap: wrap; gap: var(--s-4) var(--s-7); align-items: flex-end; }
  .res-big { display: flex; align-items: baseline; gap: var(--s-2); font-size: 48px; font-weight: 500; letter-spacing: -0.02em; line-height: 1; font-variant-numeric: tabular-nums; }
  .res-u { font-size: 17px; color: var(--muted); letter-spacing: 0; }
  .res-mid { font-size: 28px; font-weight: 500; }
  .gauges { display: grid; grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)); gap: var(--s-5); }
  .gauge { display: grid; gap: var(--s-1); }
  .g-head { display: flex; justify-content: space-between; }
  .g-track { position: relative; height: 10px; border-radius: 999px; }
  .g-track.cv {
    background: linear-gradient(90deg,
      color-mix(in srgb, var(--green-500) 28%, var(--surface)) 0 50%,
      color-mix(in srgb, var(--danger) 22%, var(--surface)) 50% 100%);
  }
  .g-track.c0 {
    background: linear-gradient(90deg,
      color-mix(in srgb, var(--danger) 22%, var(--surface)) 0 var(--lo),
      color-mix(in srgb, var(--green-500) 28%, var(--surface)) var(--lo) var(--hi),
      color-mix(in srgb, var(--danger) 22%, var(--surface)) var(--hi) 100%);
  }
  .g-mark { position: absolute; top: -4px; width: 6px; height: 18px; border-radius: 3px; background: var(--ink); }
  .g-mark.bad-mark { background: var(--danger); }
  .g-one { position: absolute; top: -3px; width: 1px; height: 16px; background: var(--muted); }
  .g-scale { display: flex; justify-content: space-between; font-size: 11px; color: var(--muted); }
  .g-scale.at { position: relative; height: 1.4em; }
  .g-scale.at span { position: absolute; transform: translateX(-50%); white-space: nowrap; }
  .flows { display: grid; gap: var(--s-2); }
  /* the label sized to its text: "Measurement 1" wrapped in the 64 px "Burst 1" had */
  .fl-head { display: flex; justify-content: space-between; align-items: center; gap: var(--s-3); }
  .f-row { display: grid; grid-template-columns: max-content 1fr 120px; gap: var(--s-3); align-items: center; font-size: 13px; }
  .f-row > span:first-child { white-space: nowrap; }
  .f-row .mono { text-align: right; }
  .f-track { height: 8px; border-radius: 999px; background: var(--surface-sunken); overflow: hidden; }
  .f-track span { display: block; height: 100%; border-radius: 999px; background: var(--teal-700); }
  .rec-row { display: flex; flex-wrap: wrap; gap: var(--s-3); align-items: flex-end; }
  .recorded {
    display: flex; flex-wrap: wrap; justify-content: space-between; align-items: center; gap: var(--s-3);
    padding: var(--s-3) var(--s-4); border-radius: var(--radius-ctl);
    background: color-mix(in srgb, var(--lime-300) 42%, var(--surface));
    border: 1px solid color-mix(in srgb, var(--green-500) 40%, var(--surface));
  }

  /* session */
  .sum { margin: 0; display: grid; grid-template-columns: auto 1fr; gap: var(--s-2) var(--s-4); font-size: 13px; }
  .sum dt { color: var(--muted); }
  .sum dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
  .side-foot { display: flex; flex-direction: column; gap: var(--s-2); padding-top: var(--s-3); border-top: 1px solid var(--line-soft); }
  .saved-note { font-size: 13px; color: var(--muted); }
  .saved-note .tick { color: var(--green-500); font-weight: 700; margin-right: 4px; }
  .abandon {
    background: none; border: none; padding: var(--s-1) 0; cursor: pointer;
    font: inherit; font-size: 13px; font-weight: 500; color: var(--danger-ink);
  }
  .abandon:hover:not(:disabled) { text-decoration: underline; }
  .abandon:disabled { opacity: 0.5; cursor: default; }

  /* abandon confirmation, as History's */
  .backdrop {
    position: fixed; inset: 0; border: none; padding: 0;
    background: rgba(18, 41, 46, 0.28); cursor: default; z-index: 60;
  }
  .layer {
    position: fixed; inset: 0; display: grid; place-items: center;
    padding: var(--s-5); z-index: 61; pointer-events: none;
  }
  .modal {
    width: 100%; max-height: 85vh; overflow-y: auto;
    box-shadow: var(--shadow-pop); pointer-events: auto;
    animation: pop 0.18s ease-out;
  }
  @keyframes pop {
    from { transform: scale(0.97); opacity: 0; }
    to { transform: scale(1); opacity: 1; }
  }
  .confirm { max-width: 440px; text-align: left; }
  .confirm h2 { font-size: 17px; margin: 0 0 var(--s-3); }
  .confirm .lede { margin: 0; padding-left: 18px; color: var(--muted); font-size: 13px; display: grid; gap: var(--s-1); }
  .confirm .lede b { color: var(--ink); }
  .modal-foot { display: flex; gap: var(--s-3); justify-content: flex-end; flex-wrap: wrap; margin-top: var(--s-5); }
  @media (prefers-reduced-motion: reduce) { .modal { animation: none; } }

  /* history */
  .hist-panel { display: flex; flex-direction: column; gap: var(--s-3); padding: var(--s-4) var(--s-5); }
  .hist-head { display: flex; justify-content: space-between; align-items: center; gap: var(--s-3); flex-wrap: wrap; }
  .hist-toggle { display: inline-flex; align-items: center; gap: var(--s-2); border: none; background: none; padding: 0; color: var(--ink); font-weight: 600; font-size: 15px; }
  .hist-toggle .k { font-weight: 400; }
  .chev { display: inline-block; color: var(--muted); transition: transform 0.15s ease; }
  .chev.open { transform: rotate(90deg); }
  .toggle { font-size: 12px; color: var(--muted); display: flex; align-items: center; gap: var(--s-1); }
  .hist-wrap { overflow-x: auto; }
  .hist { width: 100%; border-collapse: collapse; font-size: 12px; min-width: 720px; }
  .hist th { text-align: left; color: var(--muted); font-weight: 500; padding: var(--s-1) var(--s-2); }
  .hist td { padding: var(--s-1) var(--s-2); border-top: 1px solid var(--line); }
  .hist tr.archived td { color: var(--muted); }
  /* each row in its bore's pastel, a stripe on the left */
  .hist tbody tr td { background: color-mix(in srgb, var(--c-bg) 55%, var(--surface)); }
  .hist tbody tr td:first-child { box-shadow: inset 4px 0 0 var(--c-line); }
  .hist tr.archived td { background: var(--surface); }
  .hist tr.archived td:first-child { box-shadow: none; }
  .bore-tag {
    display: inline-flex; align-items: baseline; padding: 0 8px; border-radius: 999px; white-space: nowrap;
    background: var(--surface); border: 1px solid var(--c-line); color: var(--c-ink); font-weight: 600;
  }
  .bore-tag .sec { font-weight: 400; }
  .hist .row-action { text-align: right; white-space: nowrap; }
  .tag { font-size: 11px; margin-right: var(--s-2); color: var(--muted); }
  @media (prefers-reduced-motion: reduce) {
    .blocked, .blocked.nudged { animation: none; }
    .bar span, .chev { transition: none; }
  }
</style>
