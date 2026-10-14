<script>
  import { app } from '../lib/state.svelte.js';
  import { get, post } from '../lib/api.js';
  import { num, dur, vol, unitFor, digitsFor, RPM_LIMITS, FLOW_LIMITS } from '../lib/fmt.js';
  import { canStartRun } from '../lib/link.js';
  import { radioKeys, rovingTab } from '../lib/radio.js';
  import { refillAsked, refillOn, refillToChoose } from '../lib/refill.js';
  import Icon from '../components/Icon.svelte';
  import Chart from '../components/Chart.svelte';
  import ErrorText from '../components/ErrorText.svelte';
  import Steps from '../components/Steps.svelte';
  import TubePicker from '../components/TubePicker.svelte';
  import { sizeParts, tubeLabel } from '../lib/tubesize.js';
  import { downloadSettings, readSettings } from '../lib/runfile.js';

  let f = $state({
    name: '',
    control_var: 'ml_min',
    // No default: the tube is often mounted the other way round, and a run
    // started in the wrong direction has to be redone. Chosen every time.
    direction: null,
    duration_h: '0',
    duration_m: '0',
    duration_s: '0',
    kind: 'linear',
    mode: 'endpoints',
    start: 5,
    end: 50,
    value: 10, // constant
    mu_per_hour: 0.15,
    steepness: 8,
    // fed-batch F0 calculator
    fb_x0: null,
    fb_v0: null,
    fb_yxs: null,
    fb_sf: null,
    fb_ms: null, // maintenance coefficient, optional
    fb_vmax: null,
    // The balance trim by default: with no balance set up, the run simply
    // goes pump only (see `trimOn`).
    gravimetric_trim: true,
    // how the bottle is refilled: true (refill pump, Settings), false (by
    // hand), null until chosen, like the direction
    auto_refill: null,
    // tubing calibration picked for the gravimetric trim
    tubing_calibration_id: null,
    // who the run belongs to: its Teams alerts go to them only
    responsible: '',
  });

  // The steps, in the order the bench is set up. Every step can be opened at
  // any time: the steps guide, they do not lock.
  let step = $state(1);
  // A step turns green the moment its fields are right. None is right on
  // its defaults alone (the name and the direction are always to enter, the
  // duration starts at zero), so green always means something was entered.
  function goTo(n) {
    step = n;
  }
  // From "Still to set": to the field's step, then to the field, lit up.
  function jumpTo(it) {
    it.go?.();
    requestAnimationFrame(() => {
      const el = document.getElementById(it.target);
      if (!el) return;
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
      (el.querySelector('input:not(:disabled), select:not(:disabled)') ?? el.querySelector('button:not(:disabled)'))?.focus({ preventScroll: true });
      el.classList.remove('flash-target');
      void el.offsetWidth;
      el.classList.add('flash-target');
      setTimeout(() => el.classList.remove('flash-target'), 1700);
    });
  }
  // The trim came from a run or a file, not from the default: worth saying
  // so when this PC has no balance to do it.
  let trimFromSeed = $state(false);
  // A tube chosen here, or brought by a run or a file: the last run's tube
  // is then not put in its place. Declared before the seed below sets it.
  let tubeTouched = false;

  // The last person who started a run on this PC, as a starting choice.
  const LAST_RESPONSIBLE = 'ft-last-responsible';
  try {
    f.responsible = localStorage.getItem(LAST_RESPONSIBLE) ?? '';
  } catch {
    /* private mode: no default */
  }

  // "Run again" from History drops a seed here; apply it once, then clear.
  // A run already set up opens on its check: only the direction is left.
  if (app.prefill) {
    Object.assign(f, app.prefill);
    app.prefill = null;
    // The mounting may have changed since that run: chosen again, never copied.
    f.direction = null;
    trimFromSeed = !!f.gravimetric_trim;
    tubeTouched = true;
    step = 4;
  }

  // Settings file: export the form as it stands, or fill it from a file
  // exported here or from History. A responsible not set up on this PC is
  // dropped, as for a remembered one.
  let importMsg = $state(null);
  let importErr = $state(null);
  let fileInput;
  async function importSettings(e) {
    const file = e.currentTarget.files?.[0];
    e.currentTarget.value = '';
    if (!file) return;
    importMsg = null;
    importErr = null;
    try {
      const got = readSettings(await file.text());
      if (got.responsible != null && !people.some((n) => n.toLowerCase() === got.responsible.trim().toLowerCase())) {
        got.responsible = '';
      }
      // As for "Run again": the direction is chosen on the bench, not in a file.
      delete got.direction;
      Object.assign(f, got);
      f.direction = null;
      trimFromSeed = !!f.gravimetric_trim;
      tubeTouched = true;
      importMsg = `Settings loaded from ${file.name}`;
      step = 4;
    } catch (err) {
      importErr = `${file.name}: ${err.message}`;
    }
  }

  let pumpAddr = $state(1);
  let scaleConfigured = $state(false);
  let scaleDensity = $state(1);
  let scaleOnFeed = $state(true);
  let people = $state([]);
  $effect(() => {
    if (!app.connected) return;
    get('/api/config')
      .then((c) => {
        pumpAddr = c.pump.address;
        scaleConfigured = (c.scale?.path?.length ?? 0) > 0;
        scaleDensity = c.scale?.density_g_per_ml > 0 ? c.scale.density_g_per_ml : 1;
        scaleOnFeed = (c.scale?.position ?? 'feed') === 'feed';
        people = (c.notify?.people ?? []).map((p) => p.name);
        // A remembered name no longer in Settings is no choice at all.
        if (!people.some((n) => n.toLowerCase() === f.responsible.trim().toLowerCase())) f.responsible = '';
      })
      .catch(() => {});
  });
  // Required once anyone is set up for notifications (the daemon checks too).
  const needsResponsible = $derived(people.length > 0 && !f.responsible);
  // The trim only applies with a balance configured; a "Run again" of a
  // trimmed run on a PC without one keeps the wish but shows why it is off.
  const trimOn = $derived(scaleConfigured && f.gravimetric_trim);
  const autoRefill = $derived(refillOn({ wanted: f.auto_refill, trimOn, scaleOnFeed }));
  const refillShown = $derived(refillAsked({ trimOn, scaleOnFeed }));
  const refillNeeded = $derived(refillToChoose({ wanted: f.auto_refill, trimOn, scaleOnFeed }));
  const scaleLive = $derived(app.status?.scale_connected === true);
  const scaleW = $derived(app.status?.scale_weight_g ?? null);

  function openBalanceSettings() {
    app.settingsSection = 'balance';
    app.route = 'settings';
  }

  const unit = $derived(unitFor(f.control_var));
  const lim = $derived(f.control_var === 'ml_min' ? FLOW_LIMITS : RPM_LIMITS);
  const durationS = $derived(
    Math.max(
      0,
      Math.round((+f.duration_h || 0) * 3600 + (+f.duration_m || 0) * 60 + (+f.duration_s || 0)),
    ),
  );

  // Duration cells: a lone "0" clears on focus so you can type straight over
  // it; an empty cell falls back to "0" on blur.
  function durFocus(field) {
    if (f[field] === '0' || f[field] === 0) f[field] = '';
  }
  function durBlur(field) {
    if (String(f[field] ?? '').trim() === '') f[field] = '0';
  }

  // Fractional hours -> whole h / m / s, for filling the duration fields.
  function splitHMS(hoursFloat) {
    let s = Math.max(0, Math.round((hoursFloat || 0) * 3600));
    const h = Math.floor(s / 3600);
    s -= h * 3600;
    const m = Math.floor(s / 60);
    s -= m * 60;
    return { h, m, s };
  }

  // Exponential fed-batch feed that sustains growth at µ, with the substrate
  // balance at S ≈ 0 (feed-limiting):
  //   F(t) = (µ/Yx/s + ms)·X0·V0/Sf · e^(µt)
  // ms is the maintenance coefficient (g substrate per g biomass per h);
  // leave it blank for the growth-only form. Assumes µ and Yx/s constant.
  const fb = $derived.by(() => {
    const mu = Number(f.mu_per_hour);
    const x0 = Number(f.fb_x0);
    const v0 = Number(f.fb_v0);
    const y = Number(f.fb_yxs);
    const sf = Number(f.fb_sf);
    if (!(mu > 0 && x0 > 0 && v0 > 0 && y > 0 && sf > 0)) return null;
    const ms = f.fb_ms > 0 ? Number(f.fb_ms) : 0;
    const f0_Lh = ((mu / y + ms) * x0 * v0) / sf;
    const f0_mlmin = (f0_Lh * 1000) / 60;
    const vmax = Number(f.fb_vmax);
    const tmax = vmax > v0 ? (1 / mu) * Math.log(1 + (mu * (vmax - v0)) / f0_Lh) : null;
    return { f0_Lh, f0_mlmin, tmax, ms };
  });

  function useF0() {
    if (!fb) return;
    f.control_var = 'ml_min';
    f.start = Number(fb.f0_mlmin.toFixed(3));
    if (fb.tmax) {
      const { h, m, s } = splitHMS(fb.tmax);
      f.duration_h = String(h);
      f.duration_m = String(m);
      f.duration_s = String(s);
    }
  }

  function curveSpec() {
    const isConst = f.kind === 'constant';
    const start = isConst ? Number(f.value) : Number(f.start);
    const end = isConst ? Number(f.value) : Number(f.end);
    let params;
    if (f.kind === 'linear') params = { kind: 'linear', rate_per_hour: 0 };
    else if (f.kind === 'exponential') params = { kind: 'exponential', mu_per_hour: Number(f.mu_per_hour) };
    else if (f.kind === 'sigmoid')
      params = { kind: 'sigmoid', steepness: Number(f.steepness), midpoint_frac: 0.5, k_per_hour: 1, midpoint_hours: 0 };
    else params = { kind: 'constant' };
    return {
      mode: f.kind === 'exponential' ? f.mode : 'endpoints',
      start,
      end,
      duration: durationS,
      // Safety clamp = the pump's own limits. The engine re-applies this anyway.
      clamp_min: lim.min,
      clamp_max: lim.max,
      params,
    };
  }

  let preview = $state([]);
  let previewErr = $state(null);
  let debounce;
  $effect(() => {
    const spec = curveSpec();
    clearTimeout(debounce);
    debounce = setTimeout(async () => {
      try {
        const r = await post('/api/preview', { curve: spec, samples: 200 });
        preview = r.series;
        previewErr = null;
      } catch (e) {
        preview = [];
        previewErr = e.message;
      }
    }, 250);
    return () => clearTimeout(debounce);
  });

  // Recorded tubing calibrations, newest first. Only active ones made in the
  // run's own unit can seed it (the daemon refuses the others).
  let calibrations = $state([]);
  let calsLoaded = $state(false);
  $effect(() => {
    if (!app.connected) return;
    get('/api/calibrations')
      .then((rows) => (calibrations = rows ?? []))
      .catch(() => (calibrations = []))
      .finally(() => (calsLoaded = true));
  });
  // An rpm run takes rpm calibrations. An ml/min run takes ml/min ones (they
  // only seed the trim, so only with the trim on) and rpm ones: those drive
  // the pump in rpm, with or without a balance.
  const calMatches = $derived(
    calibrations.filter(
      (c) =>
        !c.archived_at &&
        (f.control_var === 'rpm'
          ? c.control_var === 'rpm'
          : c.control_var === 'rpm' || (trimOn && c.control_var === 'ml_min')),
    ),
  );
  const chosenCal = $derived(calMatches.find((c) => c.id === f.tubing_calibration_id) ?? null);
  // ml/min asked, rpm written: the pump's own head/tubing table is not used.
  const drivePerRpm = $derived(
    f.control_var === 'ml_min' && chosenCal?.control_var === 'rpm'
      ? chosenCal.mean_measured_ml_min / chosenCal.setpoint
      : null,
  );
  const showCal = $derived(trimOn || (f.control_var === 'ml_min' && calMatches.length > 0));
  // Drop a selection that no longer matches the run's unit. Only once the list
  // has loaded: before that, a calibration seeded by "Run again" would look
  // unknown and be wiped.
  $effect(() => {
    const id = f.tubing_calibration_id;
    if (calsLoaded && id != null && !calMatches.some((c) => c.id === id)) f.tubing_calibration_id = null;
  });
  const needsCalibration = $derived(
    trimOn && f.control_var === 'rpm' && f.tubing_calibration_id == null,
  );
  // The tube of the last run that named one: most often still the one on the
  // pump, so it is picked to start with, until a choice is made here (or a
  // run or a file brings its own).
  let lastUsedCal = $state(null);
  $effect(() => {
    if (!app.connected) return;
    get('/api/runs?limit=50')
      .then((rows) => {
        lastUsedCal = (rows ?? []).find((r) => r.kind === 'dosing' && r.tubing_calibration_id != null)?.tubing_calibration_id ?? null;
      })
      .catch(() => {});
  });
  $effect(() => {
    if (tubeTouched || !calsLoaded || lastUsedCal == null || f.tubing_calibration_id != null) return;
    if (calMatches.some((c) => c.id === lastUsedCal)) f.tubing_calibration_id = lastUsedCal;
  });
  const tubeName = (c) => {
    const sz = sizeParts(c.inner_diameter_mm, c.outer_diameter_mm, app.tubeSizes);
    return `${tubeLabel(c)}, Ø ${sz.pri} (${sz.sec})`;
  };

  // How much feed the curve takes: its flow summed over time (trapezoids on
  // the preview's 200 points). An rpm curve needs the tube's ml per rpm.
  const mlPerRpm = $derived(
    chosenCal?.control_var === 'rpm' && chosenCal.setpoint > 0 ? chosenCal.mean_measured_ml_min / chosenCal.setpoint : null,
  );
  const flowOf = (v) => (f.control_var === 'ml_min' ? v : mlPerRpm != null ? v * mlPerRpm : null);
  const feed = $derived.by(() => {
    if (previewErr || preview.length < 2) return null;
    let ml = 0;
    let emptyAtS = null;
    // the bottle on the balance, in mL of feed: when the curve has drawn it
    const onBalanceMl = scaleOnFeed && scaleLive && scaleW > 0 ? scaleW / scaleDensity : null;
    for (let i = 1; i < preview.length; i++) {
      const a = flowOf(preview[i - 1][1]);
      const b = flowOf(preview[i][1]);
      if (a == null || b == null) return { unknown: true };
      ml += ((a + b) / 2) * ((preview[i][0] - preview[i - 1][0]) / 60);
      if (emptyAtS == null && onBalanceMl != null && ml >= onBalanceMl) emptyAtS = preview[i][0];
    }
    // past the curve the run holds its end value until Stop
    const holdMlH = flowOf(preview[preview.length - 1][1]) * 60;
    return { ml, onBalanceMl, emptyAtS, holdMlH };
  });

  let starting = $state(false);
  let startErr = $state(null);

  async function start() {
    starting = true;
    startErr = null;
    try {
      await post('/api/runs', {
        name: f.name.trim(),
        control_var: f.control_var,
        direction: f.direction,
        pump_addr: pumpAddr,
        curve: curveSpec(),
        gravimetric_trim: trimOn,
        auto_refill: autoRefill,
        tubing_calibration_id: trimOn || drivePerRpm != null ? f.tubing_calibration_id : null,
        responsible: f.responsible || null,
      });
      try {
        if (f.responsible) localStorage.setItem(LAST_RESPONSIBLE, f.responsible);
      } catch {
        /* private mode */
      }
      app.tab = 'overview';
    } catch (e) {
      startErr = { message: e.message, hint: e.hint ?? null };
      starting = false;
    }
  }

  const busy = $derived(app.status?.active != null);
  // A run drives the pump, so it needs a live link, single gate in link.js.
  const cannotStart = $derived(!canStartRun(app.status));
  const curveOk = $derived(!previewErr && durationS > 0 && preview.length > 0);

  // What has to be right before Start comes on. Each item takes you to its
  // field, on its step. The direction is never set by default (see `f`).
  const checklist = $derived([
    // A name to find the run again in History: "run" twenty times reads as nothing.
    { label: 'Run name', done: !!f.name.trim(), hint: 'To find it in History', target: 'nr-name', go: () => goTo(1) },
    ...(people.length
      ? [{ label: 'Responsible', done: !needsResponsible, hint: 'Who gets the alerts', target: 'nr-resp', go: () => goTo(1) }]
      : []),
    { label: 'Pump direction', done: f.direction === 'cw' || f.direction === 'ccw', hint: 'Check how the tube is mounted', target: 'pump-direction', go: () => goTo(2) },
    ...(refillShown
      ? [{ label: 'Bottle refill', done: !refillNeeded, hint: 'Refill pump or by hand', target: 'nr-refill', go: () => goTo(2) }]
      : []),
    ...(trimOn ? [{ label: 'Tubing calibration', done: !needsCalibration, hint: 'Needed for the regulation in rpm', target: 'nr-tube', go: () => goTo(2) }] : []),
    { label: 'Duration', done: durationS > 0, hint: 'Longer than zero', target: 'nr-duration', go: () => goTo(3) },
    { label: 'Curve', done: curveOk, hint: 'A valid profile', target: 'nr-shape', go: () => goTo(3) },
    busy
      ? { label: 'No other run', done: false, hint: 'Stop it from Overview first' }
      : { label: 'Pump connected', done: !cannotStart, hint: 'Connect the pump', target: 'connbar' },
  ]);
  const ready = $derived(checklist.every((i) => i.done));

  const KINDS = [
    { v: 'linear', label: 'Linear', d: 'M4 26 L56 4' },
    { v: 'exponential', label: 'Exponential', d: 'M4 26 C30 26 46 22 56 4' },
    { v: 'sigmoid', label: 'Sigmoid', d: 'M4 26 C26 26 34 4 56 4' },
    { v: 'constant', label: 'Constant', d: 'M4 15 L56 15' },
  ];
  const kindLabel = $derived(KINDS.find((k) => k.v === f.kind)?.label ?? f.kind);
  const digits = $derived(digitsFor(f.control_var));
  const startV = $derived(preview.length ? preview[0][1] : null);
  const endV = $derived(preview.length ? preview[preview.length - 1][1] : null);

  const steps = $derived([
    { n: 1, label: 'Run', sub: f.name.trim() || (f.responsible ? `for ${f.responsible}` : 'name, responsible'), done: !!f.name.trim() && !needsResponsible },
    {
      n: 2,
      label: 'Pump and tube',
      sub: `${unit}, ${f.direction ?? 'direction to choose'}, ${trimOn ? 'balance regulation' : 'no regulation'}${autoRefill ? ', automatic refill' : ''}`,
      done: !!f.direction && !needsCalibration && !refillNeeded,
    },
    { n: 3, label: 'Feed profile', sub: durationS > 0 ? `${kindLabel}, ${dur(durationS)}` : kindLabel, done: curveOk },
    { n: 4, label: 'Check and start', sub: `${checklist.filter((i) => i.done).length} of ${checklist.length} ready` },
  ]);
  const NEXT = { 1: 'Next: pump and tube', 2: 'Next: feed profile', 3: 'Next: check and start' };
  const BACK = { 2: 'Run', 3: 'Pump and tube', 4: 'Feed profile' };
</script>

<div class="nr">
  <header class="nr-head">
    <div><div class="eyebrow">Run setup</div><h2>New run</h2></div>
    <div class="file-tools">
      <input type="file" accept=".json,application/json" hidden bind:this={fileInput} onchange={importSettings} />
      <button class="btn-ghost" onclick={() => fileInput.click()} title="Fill the form from a settings file">Import settings</button>
      <button class="btn-ghost" onclick={() => downloadSettings(f)} title="Save the form to a file, to import later">Export settings</button>
    </div>
  </header>
  {#if importErr}<div class="err">{importErr}</div>{/if}
  {#if importMsg}<p class="imported mono">{importMsg}: check it, then choose the pump direction.</p>{/if}
  {#if busy}
    <div class="err">A run is already active. Stop it from Overview first.</div>
  {/if}

  <Steps label="New run steps" {steps} current={step} onpick={goTo} />

  <section class="card panel">
    {#if step === 1}
      <h3>Run</h3>
      <div class="row">
        <label class="field grow" id="nr-name"><span>Run name{#if !f.name.trim()}<span class="choose">required</span>{/if}</span>
          <input type="text" bind:value={f.name} placeholder="e.g. ferment-A2" class:missing={!f.name.trim()} />
        </label>
        {#if people.length}
          <label class="field resp" id="nr-resp"><span>Responsible{#if needsResponsible}<span class="choose">choose</span>{/if}</span>
            <select bind:value={f.responsible} class:missing={needsResponsible}>
              <option value="" disabled>Who gets the alerts?</option>
              {#each people as n}<option value={n}>{n}</option>{/each}
            </select>
          </label>
        {/if}
      </div>
      <p class="k">
        {people.length
          ? "The run's alerts (alarms, link lost, end of curve) go to this person only."
          : 'No one is set up for alerts on this PC (Settings, Notifications).'}
      </p>
    {:else if step === 2}
      <h3>Pump and tube</h3>
      <div class="row">
        <div class="field"><span>Set the pump in</span>
          <div class="seg" role="radiogroup" aria-label="Pump unit" tabindex="-1" onkeydown={radioKeys}>
            <button role="radio" aria-checked={f.control_var === 'ml_min'} tabindex={rovingTab(f.control_var === 'ml_min', false)}
              class:on={f.control_var === 'ml_min'} onclick={() => (f.control_var = 'ml_min')}>ml/min</button>
            <button role="radio" aria-checked={f.control_var === 'rpm'} tabindex={rovingTab(f.control_var === 'rpm', false)}
              class:on={f.control_var === 'rpm'} onclick={() => (f.control_var = 'rpm')}>rpm</button>
          </div>
        </div>
        <div class="field"><span>Pump direction{#if !f.direction}<span class="choose">choose</span>{/if}</span>
          <div class="seg" class:need={!f.direction} id="pump-direction" role="radiogroup" aria-label="Pump direction" tabindex="-1" onkeydown={radioKeys}>
            <button role="radio" aria-checked={f.direction === 'cw'} tabindex={rovingTab(f.direction === 'cw', !f.direction)}
              class:on={f.direction === 'cw'} title="Clockwise" onclick={() => (f.direction = 'cw')}>↻ cw</button>
            <button role="radio" aria-checked={f.direction === 'ccw'} tabindex={rovingTab(f.direction === 'ccw', false)}
              class:on={f.direction === 'ccw'} title="Counter-clockwise" onclick={() => (f.direction = 'ccw')}>↺ ccw</button>
          </div>
        </div>
        <div class="field"><span>Regulation</span>
          <div class="seg" role="radiogroup" aria-label="Regulation" tabindex="-1" onkeydown={radioKeys}>
            <button role="radio" aria-checked={trimOn} tabindex={rovingTab(trimOn, false)} class:on={trimOn}
              disabled={!scaleConfigured} title={scaleConfigured ? 'The balance corrects the pump' : 'No balance set up'}
              onclick={() => (f.gravimetric_trim = true)}><span class="opt"><Icon name="weight" size={15} />Balance regulation</span></button>
            <button role="radio" aria-checked={!trimOn} tabindex={rovingTab(!trimOn, false)} class:on={!trimOn}
              title="The pump runs on its own" onclick={() => (f.gravimetric_trim = false)}><span class="opt"><Icon name="off" size={15} />No regulation</span></button>
          </div>
        </div>
        {#if refillShown}
          {@const byHand = f.auto_refill === false}
          <div class="field"><span>Bottle refill{#if refillNeeded}<span class="choose">choose</span>{/if}</span>
            <div class="seg" class:need={refillNeeded} id="nr-refill" role="radiogroup" aria-label="Bottle refill" tabindex="-1" onkeydown={radioKeys}>
              <button role="radio" aria-checked={autoRefill} tabindex={rovingTab(autoRefill, refillNeeded)} class:on={autoRefill}
                title="The refill pump tops the weighed bottle up from the reservoir" onclick={() => (f.auto_refill = true)}><span class="opt"><Icon name="droplet" size={15} />Refill pump</span></button>
              <button role="radio" aria-checked={byHand} tabindex={rovingTab(byHand, false)} class:on={byHand}
                title="The bottle is refilled by hand" onclick={() => (f.auto_refill = false)}><span class="opt"><Icon name="hand" size={15} />By hand</span></button>
            </div>
          </div>
        {/if}
      </div>
      {#if !scaleConfigured}
        <p class="k">
          {trimFromSeed && f.gravimetric_trim ? 'This run used the balance regulation, but no balance is set up here.' : 'The regulation needs a balance.'}
          <button class="linkish" type="button" onclick={openBalanceSettings}>Set up a balance</button>
        </p>
      {:else if trimOn && !scaleLive}
        <p class="k bad">The balance is not responding: the regulation cannot correct anything until it is back.</p>
      {/if}

      {#if showCal}
        <div class="field" id="nr-tube"><span>Tube on the pump</span>
          <TubePicker cals={calMatches} value={f.tubing_calibration_id} lastUsed={lastUsedCal}
            onchange={(id) => { f.tubing_calibration_id = id; tubeTouched = true; }}
            allowNone={f.control_var === 'ml_min'}
            noneText={trimOn ? "The pump's own ml/min, the regulation starts at ×1.00" : "The pump's own ml/min"}
            oncalibrate={() => (app.tab = 'calibration')} />
          {#if chosenCal && Date.now() - Date.parse(chosenCal.created_at) > 30 * 86_400_000}
            <p class="k warn-t">
              Calibrated over 30 days ago: tubing wears and its flow drifts. Recalibrate it before a long run.
              <button class="linkish" type="button" onclick={() => (app.tab = 'calibration')}>Calibrate this tube</button>
            </p>
          {/if}
          {#if needsCalibration}
            <p class="k bad">
              {calMatches.length ? 'Pick the tube you installed:' : `No ${unit} calibration recorded yet:`}
              the regulation cannot start in rpm without one.
              <button class="linkish" type="button" onclick={() => (app.tab = 'calibration')}>Calibrate this tube</button>
            </p>
          {:else if trimOn && f.control_var === 'ml_min' && f.tubing_calibration_id == null}
            <p class="k">
              Without a calibration the regulation starts at ×1.00 and corrects from there.
              <button class="linkish" type="button" onclick={() => (app.tab = 'calibration')}>Calibrate this tube</button>
            </p>
          {/if}
        </div>
      {/if}
    {:else if step === 3}
      <h3>Feed profile</h3>
      <div class="profile">
        <div class="profile-form">
          <div class="field"><span>Shape</span>
            <div class="shapes" id="nr-shape" role="radiogroup" aria-label="Curve shape" tabindex="-1" onkeydown={radioKeys}>
              {#each KINDS as k (k.v)}
                <button class="shape" role="radio" aria-checked={f.kind === k.v} tabindex={rovingTab(f.kind === k.v, false)}
                  class:on={f.kind === k.v} onclick={() => (f.kind = k.v)}>
                  <svg viewBox="0 0 60 30" aria-hidden="true"><path d={k.d} /></svg>
                  <span>{k.label}</span>
                </button>
              {/each}
            </div>
          </div>

          <div class="row">
            {#if f.kind === 'constant'}
              <label class="field num-f"><span>Speed, {unit}</span>
                <input type="number" step="0.1" bind:value={f.value} />
              </label>
            {:else}
              <label class="field num-f"><span>Start, {unit}</span>
                <input type="number" step="0.1" bind:value={f.start} />
              </label>
              {#if !(f.kind === 'exponential' && f.mode === 'physio')}
                <label class="field num-f"><span>End, {unit}</span>
                  <input type="number" step="0.1" bind:value={f.end} />
                </label>
              {/if}
            {/if}
            {#if f.kind === 'exponential'}
              <div class="field"><span>Set by</span>
                <div class="seg" role="radiogroup" aria-label="Exponential set by" tabindex="-1" onkeydown={radioKeys}>
                  <button role="radio" aria-checked={f.mode === 'endpoints'} tabindex={rovingTab(f.mode === 'endpoints', false)}
                    class:on={f.mode === 'endpoints'} onclick={() => (f.mode = 'endpoints')}>start → end</button>
                  <button role="radio" aria-checked={f.mode === 'physio'} tabindex={rovingTab(f.mode === 'physio', false)}
                    class:on={f.mode === 'physio'} onclick={() => (f.mode = 'physio')}>rate µ</button>
                </div>
              </div>
              {#if f.mode === 'physio'}
                <label class="field num-f"><span>µ, per hour</span>
                  <input type="number" step="0.01" bind:value={f.mu_per_hour} />
                </label>
              {/if}
            {/if}
            {#if f.kind === 'sigmoid'}
              <label class="field num-f"><span>Steepness</span>
                <input type="number" step="1" min="1" bind:value={f.steepness} />
              </label>
            {/if}
          </div>

          <div class="field" id="nr-duration"><span>Duration</span>
            <div class="dur">
              <label><input type="text" inputmode="numeric" autocomplete="off" aria-label="Hours"
                bind:value={f.duration_h}
                onfocus={() => durFocus('duration_h')}
                onblur={() => durBlur('duration_h')}
                oninput={(e) => (f.duration_h = e.target.value.replace(/[^0-9]/g, ''))} /><span>h</span></label>
              <label><input type="text" inputmode="numeric" autocomplete="off" aria-label="Minutes"
                bind:value={f.duration_m}
                onfocus={() => durFocus('duration_m')}
                onblur={() => durBlur('duration_m')}
                oninput={(e) => (f.duration_m = e.target.value.replace(/[^0-9]/g, ''))} /><span>min</span></label>
              <label><input type="text" inputmode="numeric" autocomplete="off" aria-label="Seconds"
                bind:value={f.duration_s}
                onfocus={() => durFocus('duration_s')}
                onblur={() => durBlur('duration_s')}
                oninput={(e) => (f.duration_s = e.target.value.replace(/[^0-9]/g, ''))} /><span>s</span></label>
            </div>
          </div>
        </div>

        <div class="preview">
          {#if previewErr}
            <div class="err">{previewErr}</div>
          {:else if preview.length && durationS > 0}
            <Chart planned={preview} actual={[]} nowS={null} {durationS} {unit} {digits} />
            <div class="pv-cap mono">
              <span>start <b>{num(startV, digits)}</b></span>
              <span>end <b>{num(endV, digits)}</b> {unit}</span>
              <span><b>{dur(durationS)}</b></span>
            </div>
            {@render feedNote()}
          {:else}
            <p class="k">Give the curve a duration to see it.</p>
          {/if}
        </div>
      </div>

      {#if f.kind === 'exponential' && f.mode === 'physio'}
        <details class="fb">
          <summary>Fed-batch F₀ from strain parameters</summary>
          <p class="fb-eq mono">F₀ = (µ / Y<sub>x/s</sub> + m<sub>s</sub>) · X₀ · V₀ / S<sub>f</sub></p>
          <p class="fb-note">
            Assumes µ and Y<sub>x/s</sub> constant and the culture substrate-limiting (S ≈ 0).
            Leave m<sub>s</sub> blank for the growth-only form.
          </p>
          <div class="grid">
            <label class="field"><span>X₀ | biomass at feed start (g/L)</span>
              <input type="number" step="0.1" bind:value={f.fb_x0} placeholder="e.g. 2" />
            </label>
            <label class="field"><span>V₀ | culture volume (L)</span>
              <input type="number" step="0.1" bind:value={f.fb_v0} placeholder="e.g. 1.0" />
            </label>
            <label class="field"><span>Y<sub>x/s</sub> | yield (g/g)</span>
              <input type="number" step="0.01" bind:value={f.fb_yxs} placeholder="E. coli/glucose ≈ 0.45" />
            </label>
            <label class="field"><span>S<sub>f</sub> | substrate in feed bottle (g/L)</span>
              <input type="number" step="1" bind:value={f.fb_sf} placeholder="e.g. 500" />
            </label>
            <label class="field"><span>m<sub>s</sub> | maintenance (g/g·h, optional)</span>
              <input type="number" step="0.001" bind:value={f.fb_ms} placeholder="E. coli ≈ 0.025" />
            </label>
            <label class="field"><span>V<sub>max</sub> | reactor limit (L, optional)</span>
              <input type="number" step="0.1" bind:value={f.fb_vmax} placeholder="for the t_max hint" />
            </label>
          </div>

          {#if fb}
            <div class="fb-out mono">
              F₀ = <b>{fb.f0_mlmin.toFixed(3)} ml/min</b>
              <span class="dim">({fb.f0_Lh.toFixed(4)} L/h | µ = {f.mu_per_hour} h⁻¹{fb.ms ? ` | m_s = ${fb.ms}` : ''})</span>
              {#if fb.tmax}<br />reaches V<sub>max</sub> in ≈ <b>{fb.tmax.toFixed(1)} h</b>{/if}
            </div>
            <button class="btn-ghost" type="button" onclick={useF0}>
              Use → Control ml/min, Start {fb.f0_mlmin.toFixed(2)}{fb.tmax ? `, Duration ${dur(fb.tmax * 3600)}` : ''}
            </button>
          {:else}
            <p class="k">Fill µ, X₀, V₀, Y<sub>x/s</sub> and S<sub>f</sub> (all &gt; 0).</p>
          {/if}
        </details>
      {/if}
    {:else}
      <h3>Check and start</h3>
      <div class="review">
        <dl class="recap">
          <dt>Run</dt>
          <dd>{#if f.name.trim()}{f.name.trim()}{:else}<b class="todo">name to enter</b>{/if}{f.responsible ? `, alerts to ${f.responsible}` : ''}</dd>
          <dd class="edit"><button class="linkish" onclick={() => goTo(1)}>Change</button></dd>

          <dt>Pump</dt>
          <dd>
            {unit},
            {#if f.direction}{f.direction === 'cw' ? '↻ cw' : '↺ ccw'}{:else}<b class="todo">direction to choose</b>{/if},
            {trimOn ? 'balance regulation' : 'no regulation'}{autoRefill ? ', automatic refill' : ''}
          </dd>
          <dd class="edit"><button class="linkish" onclick={() => goTo(2)}>Change</button></dd>

          <dt>Tube</dt>
          <dd>{chosenCal ? tubeName(chosenCal) : needsCalibration ? '' : 'no calibration'}{#if needsCalibration}<b class="todo">calibration to choose</b>{/if}</dd>
          <dd class="edit"><button class="linkish" onclick={() => goTo(2)}>Change</button></dd>

          <dt>Curve</dt>
          <dd>
            {#if curveOk}{kindLabel}, {num(startV, digits)} to {num(endV, digits)} {unit}, {dur(durationS)}
            {:else}<b class="todo">{previewErr ? 'not valid' : 'no duration yet'}</b>{/if}
          </dd>
          <dd class="edit"><button class="linkish" onclick={() => goTo(3)}>Change</button></dd>

          <dt>Feed</dt>
          <dd>{feed && !feed.unknown ? vol(feed.ml) : '–'}</dd>
          <dd class="edit"></dd>
        </dl>
        <div class="go">
          <!-- the curve the pump will follow, the last thing seen before Start -->
          {#if curveOk}
            <div class="preview">
              <Chart planned={preview} actual={[]} nowS={null} {durationS} {unit} {digits} />
              <div class="pv-cap mono">
                <span>start <b>{num(startV, digits)}</b></span>
                <span>end <b>{num(endV, digits)}</b> {unit}</span>
                <span><b>{dur(durationS)}</b></span>
              </div>
            </div>
          {/if}
          {#if startErr}
            <ErrorText message={startErr.message} hint={startErr.hint} />
          {/if}
          <button class="btn-primary start" disabled={starting || !ready} onclick={start}>
            {starting ? 'Starting…' : 'Start run'}
          </button>
          {#if !ready}
            <!-- what is missing, each a way to its field -->
            <p class="k center">
              Still to set:
              {#each checklist.filter((i) => !i.done) as it, i (it.label)}{i ? ', ' : ''}{#if it.target}<button
                    class="linkish" type="button" onclick={() => jumpTo(it)}>{it.label.toLowerCase()}</button
                  >{:else}{it.label.toLowerCase()}{/if}{/each}.
            </p>
          {/if}
        </div>
      </div>
    {/if}

    <div class="nav-row">
      {#if BACK[step]}
        <button class="btn-ghost" onclick={() => goTo(step - 1)}>← {BACK[step]}</button>
      {:else}
        <span></span>
      {/if}
      {#if NEXT[step]}
        <button class="btn-primary" onclick={() => goTo(step + 1)}>{NEXT[step]} →</button>
      {/if}
    </div>
  </section>
</div>

{#snippet feedNote()}
  {#if feed?.unknown}
    <p class="feed k">Feed needed: pick the tube, its ml per rpm turns the curve into mL.</p>
  {:else if feed}
    <div class="feed">
      <span class="k">Feed needed for the curve</span>
      <b class="mono">{vol(feed.ml)}</b>
      {#if feed.onBalanceMl != null}
        {#if feed.emptyAtS != null}
          <span class="k warn-t">More than the {num(scaleW, 0)} g on the balance: refill around {dur(feed.emptyAtS)} in.</span>
        {:else}
          <span class="k">{num(scaleW, 0)} g on the balance, about {vol(feed.onBalanceMl - feed.ml)} to spare.</span>
        {/if}
      {/if}
      <span class="k">Then {vol(feed.holdMlH)} an hour while it holds its end value, until you stop it.</span>
    </div>
  {/if}
{/snippet}

<style>
  /* a pictogram before the words of a choice */
  .opt { display: inline-flex; align-items: center; gap: 6px; }
  .nr { display: flex; flex-direction: column; gap: var(--s-4); }
  .nr-head { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: flex-end; gap: var(--s-3); }
  .nr-head h2 { font-size: 22px; }
  .file-tools { display: flex; gap: var(--s-2); flex-wrap: wrap; }
  .imported { margin: 0; font-size: 12px; color: var(--muted); }

  .panel { display: flex; flex-direction: column; gap: var(--s-5); }
  .panel h3 { margin: 0; font-size: 17px; }
  .row { display: flex; flex-wrap: wrap; gap: var(--s-4); align-items: flex-start; }
  .grow { flex: 1 1 260px; }
  .resp { flex: 0 1 240px; }
  .k { margin: 0; font-size: 13px; color: var(--muted); line-height: 1.45; }
  .k.bad { color: var(--danger); }
  .center { text-align: center; }
  .linkish {
    background: none; border: none; padding: 0; font: inherit;
    color: var(--teal-700); text-decoration: underline; cursor: pointer;
  }
  select.missing, input.missing { border-color: var(--measured); }

  /* feed profile */
  .profile { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 380px); gap: var(--s-5); align-items: start; }
  @media (max-width: 900px) { .profile { grid-template-columns: 1fr; } }
  .profile-form { display: flex; flex-direction: column; gap: var(--s-4); min-width: 0; }
  .shapes { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: var(--s-2); max-width: 520px; }
  @media (max-width: 520px) { .shapes { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
  .shape {
    display: flex; flex-direction: column; align-items: center; gap: 4px;
    padding: var(--s-2) var(--s-2) 6px; border: 1px solid var(--line); border-radius: var(--radius-ctl);
    background: var(--surface); color: var(--muted); cursor: pointer;
  }
  .shape svg { width: 100%; height: 32px; }
  .shape path { fill: none; stroke: currentColor; stroke-width: 2.2; stroke-linecap: round; stroke-linejoin: round; }
  .shape span { font-size: 12.5px; color: var(--ink); }
  .shape.on { border-color: var(--teal-700); box-shadow: inset 0 0 0 1px var(--teal-700); color: var(--teal-700); }
  .shape.on span { font-weight: 600; }
  .num-f input { width: 120px; font-family: var(--mono); }
  .dur { display: flex; gap: var(--s-3); }
  .dur label { display: flex; align-items: baseline; gap: var(--s-2); }
  .dur input { width: 70px; text-align: right; font-family: var(--mono); }
  .dur label > span { font-size: 12px; color: var(--muted); }
  .preview { background: var(--surface-sunken); border-radius: var(--radius-ctl); padding: var(--s-3) var(--s-4); min-width: 0; display: flex; flex-direction: column; gap: var(--s-2); }
  .pv-cap { display: flex; flex-wrap: wrap; gap: 4px var(--s-4); font-size: 12px; color: var(--muted); }
  .pv-cap b { color: var(--ink); font-weight: 500; }
  .feed { display: flex; flex-direction: column; gap: 2px; padding-top: var(--s-2); border-top: 1px solid var(--line); }
  .feed b { font-size: 15px; font-weight: 600; }
  .feed .k { font-size: 12.5px; }
  .warn-t { color: color-mix(in srgb, var(--measured) 80%, var(--ink)); font-weight: 600; }

  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: var(--s-4); }
  .fb {
    border: 1px solid var(--line); border-radius: var(--radius-ctl);
    padding: var(--s-3) var(--s-4); background: var(--surface-sunken);
  }
  .fb summary { cursor: pointer; font-weight: 600; font-size: 13px; }
  .fb[open] summary { margin-bottom: var(--s-4); }
  .fb-eq { font-size: 12px; color: var(--muted); margin: 0 0 var(--s-2); }
  .fb-note { font-size: 11px; color: var(--muted); margin: 0 0 var(--s-4); line-height: 1.4; }
  .fb-out { font-size: 13px; margin: var(--s-4) 0; padding: var(--s-3); background: var(--surface); border-radius: var(--radius-ctl); }
  .fb-out .dim { color: var(--muted); }

  /* check and start */
  .review { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 420px); gap: var(--s-6); align-items: start; }
  @media (max-width: 900px) { .review { grid-template-columns: 1fr; } }
  .recap { margin: 0; display: grid; grid-template-columns: auto minmax(0, 1fr) auto; gap: var(--s-3) var(--s-4); font-size: 14px; align-items: baseline; }
  .recap dt { color: var(--muted); font-size: 13px; }
  .recap dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
  .recap .edit { font-size: 13px; }
  .todo { color: color-mix(in srgb, var(--measured) 80%, var(--ink)); }
  .go { display: flex; flex-direction: column; gap: var(--s-3); }
  .start { padding: var(--s-3) var(--s-4); font-size: 15px; }

  .nav-row { display: flex; justify-content: space-between; align-items: center; gap: var(--s-3); flex-wrap: wrap; padding-top: var(--s-4); border-top: 1px solid var(--line-soft); }
</style>
