<script>
  import { app } from '../lib/state.svelte.js';
  import { get, post } from '../lib/api.js';
  import { num, dur, unitFor, digitsFor, RPM_LIMITS, FLOW_LIMITS } from '../lib/fmt.js';
  import Chart from '../components/Chart.svelte';

  let f = $state({
    name: '',
    control_var: 'ml_min',
    direction: 'cw',
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
    fb_vmax: null,
  });

  // "Run again" from History drops a seed here; apply it once, then clear.
  if (app.prefill) {
    Object.assign(f, app.prefill);
    app.prefill = null;
  }

  let pumpAddr = $state(1);
  $effect(() => {
    get('/api/config')
      .then((c) => (pumpAddr = c.pump.address))
      .catch(() => {});
  });

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

  // F0 = µ·X0·V0 / (Yx/s·Sf): the feed rate that sustains growth at µ.
  const fb = $derived.by(() => {
    const mu = Number(f.mu_per_hour);
    const x0 = Number(f.fb_x0);
    const v0 = Number(f.fb_v0);
    const y = Number(f.fb_yxs);
    const sf = Number(f.fb_sf);
    if (!(mu > 0 && x0 > 0 && v0 > 0 && y > 0 && sf > 0)) return null;
    const f0_Lh = (mu * x0 * v0) / (y * sf);
    const f0_mlmin = (f0_Lh * 1000) / 60;
    const vmax = Number(f.fb_vmax);
    const tmax = vmax > v0 ? (1 / mu) * Math.log(1 + (mu * (vmax - v0)) / f0_Lh) : null;
    return { f0_Lh, f0_mlmin, tmax };
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

  let starting = $state(false);
  let startErr = $state(null);

  async function start() {
    starting = true;
    startErr = null;
    try {
      await post('/api/runs', {
        name: f.name.trim() || 'run',
        control_var: f.control_var,
        direction: f.direction,
        pump_addr: pumpAddr,
        curve: curveSpec(),
      });
      app.tab = 'overview';
    } catch (e) {
      startErr = e.message;
      starting = false;
    }
  }

  const busy = $derived(app.status?.active != null);
  // A real port is configured but not open: a run would drive nothing.
  const linkDown = $derived(app.status?.serial_ok === false);
</script>

<section class="card">
  <div class="card-head">
    <div><h2>New run</h2></div>
  </div>

  {#if busy}
    <div class="err" style="margin-bottom:16px">A run is already active. Stop it from Overview first.</div>
  {/if}

  {#if linkDown}
    <div class="err" style="margin-bottom:16px">
      Pump link is down — connect the pump before starting a run (or set the serial port to
      the simulator in Settings).
    </div>
  {/if}

  <div class="group">
    <div class="eyebrow">Run</div>
    <div class="grid">
      <label class="field"><span>Run name</span>
        <input type="text" bind:value={f.name} placeholder="e.g. ferment-A2" />
      </label>

      <div class="field dur-field"><span>Duration</span>
        <div class="dur">
          <label><input type="text" inputmode="numeric" autocomplete="off"
            bind:value={f.duration_h}
            onfocus={() => durFocus('duration_h')}
            onblur={() => durBlur('duration_h')}
            oninput={(e) => (f.duration_h = e.target.value.replace(/[^0-9]/g, ''))} /><span>h</span></label>
          <label><input type="text" inputmode="numeric" autocomplete="off"
            bind:value={f.duration_m}
            onfocus={() => durFocus('duration_m')}
            onblur={() => durBlur('duration_m')}
            oninput={(e) => (f.duration_m = e.target.value.replace(/[^0-9]/g, ''))} /><span>min</span></label>
          <label><input type="text" inputmode="numeric" autocomplete="off"
            bind:value={f.duration_s}
            onfocus={() => durFocus('duration_s')}
            onblur={() => durBlur('duration_s')}
            oninput={(e) => (f.duration_s = e.target.value.replace(/[^0-9]/g, ''))} /><span>s</span></label>
        </div>
      </div>
    </div>
  </div>

  <div class="group row">
    <div class="pane pane-pump">
      <div class="eyebrow">Pump output</div>
      <div class="pump-fields">
        <div class="field"><span>Control</span>
          <div class="seg">
            <button class:on={f.control_var === 'ml_min'} onclick={() => (f.control_var = 'ml_min')}>ml/min</button>
            <button class:on={f.control_var === 'rpm'} onclick={() => (f.control_var = 'rpm')}>rpm</button>
          </div>
        </div>

        <div class="field"><span>Direction</span>
          <div class="seg">
            <button class:on={f.direction === 'cw'} onclick={() => (f.direction = 'cw')}>clockwise</button>
            <button class:on={f.direction === 'ccw'} onclick={() => (f.direction = 'ccw')}>counter</button>
          </div>
        </div>
      </div>
    </div>

    <div class="pane pane-profile">
      <div class="eyebrow">Profile</div>
      <div class="grid">
      <label class="field"><span>Curve</span>
        <select bind:value={f.kind}>
          <option value="linear">Linear</option>
          <option value="exponential">Exponential</option>
          <option value="sigmoid">Sigmoid</option>
          <option value="constant">Constant</option>
        </select>
      </label>

      {#if f.kind === 'constant'}
        <label class="field"><span>Speed ({unit})</span>
          <input type="number" step="0.1" bind:value={f.value} />
        </label>
      {:else}
        <label class="field"><span>Start ({unit})</span>
          <input type="number" step="0.1" bind:value={f.start} />
        </label>
        {#if !(f.kind === 'exponential' && f.mode === 'physio')}
          <label class="field"><span>End ({unit})</span>
            <input type="number" step="0.1" bind:value={f.end} />
          </label>
        {/if}
      {/if}

      {#if f.kind === 'exponential'}
        <div class="field"><span>Exponential mode</span>
          <div class="seg">
            <button class:on={f.mode === 'endpoints'} onclick={() => (f.mode = 'endpoints')}>start → end</button>
            <button class:on={f.mode === 'physio'} onclick={() => (f.mode = 'physio')}>rate µ</button>
          </div>
        </div>
        {#if f.mode === 'physio'}
          <label class="field"><span>µ (per hour)</span>
            <input type="number" step="0.01" bind:value={f.mu_per_hour} />
          </label>
        {/if}
      {/if}

      {#if f.kind === 'sigmoid'}
        <label class="field"><span>Steepness</span>
          <input type="number" step="1" min="1" bind:value={f.steepness} />
        </label>
      {/if}
      </div>
    </div>
  </div>

  {#if f.kind === 'exponential' && f.mode === 'physio'}
    <details class="fb">
      <summary>Fed-batch F₀ from strain parameters</summary>
      <p class="fb-eq mono">F₀ = µ · X₀ · V₀ / (Y<sub>x/s</sub> · S<sub>f</sub>)</p>
      <div class="grid">
        <label class="field"><span>X₀ · biomass at feed start (g/L)</span>
          <input type="number" step="0.1" bind:value={f.fb_x0} placeholder="e.g. 2" />
        </label>
        <label class="field"><span>V₀ · culture volume (L)</span>
          <input type="number" step="0.1" bind:value={f.fb_v0} placeholder="e.g. 1.0" />
        </label>
        <label class="field"><span>Y<sub>x/s</sub> · yield (g/g)</span>
          <input type="number" step="0.01" bind:value={f.fb_yxs} placeholder="E. coli/glucose ≈ 0.45" />
        </label>
        <label class="field"><span>S<sub>f</sub> · feed substrate (g/L)</span>
          <input type="number" step="1" bind:value={f.fb_sf} placeholder="e.g. 500" />
        </label>
        <label class="field"><span>V<sub>max</sub> · reactor limit (L, optional)</span>
          <input type="number" step="0.1" bind:value={f.fb_vmax} placeholder="for the t_max hint" />
        </label>
      </div>

      {#if fb}
        <div class="fb-out mono">
          F₀ = <b>{fb.f0_mlmin.toFixed(3)} ml/min</b>
          <span class="dim">({fb.f0_Lh.toFixed(4)} L/h · µ = {f.mu_per_hour} h⁻¹)</span>
          {#if fb.tmax}<br />reaches V<sub>max</sub> in ≈ <b>{fb.tmax.toFixed(1)} h</b>{/if}
        </div>
        <button class="btn-ghost" type="button" onclick={useF0}>
          Use → Control ml/min, Start {fb.f0_mlmin.toFixed(2)}{fb.tmax ? `, Duration ${dur(fb.tmax * 3600)}` : ''}
        </button>
      {:else}
        <p class="muted">Fill µ, X₀, V₀, Y<sub>x/s</sub> and S<sub>f</sub> (all &gt; 0).</p>
      {/if}
    </details>
  {/if}

  <div class="preview">
    <div class="eyebrow" style="margin-bottom:8px">Preview</div>
    {#if previewErr}
      <div class="err">{previewErr}</div>
    {:else if preview.length}
      <Chart planned={preview} actual={[]} nowS={null} {durationS} {unit} digits={digitsFor(f.control_var)} />
      <div class="pv-cap mono">
        start {num(preview[0][1], digitsFor(f.control_var))} {unit} · end {num(preview[preview.length - 1][1], digitsFor(f.control_var))} {unit} · {dur(durationS)}
      </div>
    {:else}
      <p class="muted">Adjust the fields to see the curve.</p>
    {/if}
  </div>

  {#if startErr}<div class="err" style="margin-top:16px">{startErr}</div>{/if}

  <div class="foot">
    <button class="btn-primary" disabled={starting || busy || linkDown || !!previewErr || durationS <= 0} onclick={start}>
      {starting ? 'Starting…' : 'Start run'}
    </button>
  </div>
</section>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: var(--s-4);
  }
  .group + .group {
    margin-top: var(--s-5);
    padding-top: var(--s-5);
    border-top: 1px solid color-mix(in srgb, var(--muted) 32%, transparent);
  }
  .group .eyebrow { margin-bottom: var(--s-3); color: var(--ink); }

  .group.row {
    display: flex;
    gap: var(--s-5);
    align-items: stretch;
    flex-wrap: wrap;
    /* the panes carry the vertical padding so the divider spans hairline to hairline */
    padding-top: 0;
    padding-bottom: 0;
    border-bottom: 1px solid color-mix(in srgb, var(--muted) 32%, transparent);
  }
  .group.row > .pane { padding-block: var(--s-5); }
  .group.row + .preview {
    margin-top: 0;
    padding-top: var(--s-6);
    border-top: none;
  }
  .pane-pump {
    flex: 0 0 auto;
    padding-right: var(--s-5);
    border-right: 1px solid color-mix(in srgb, var(--muted) 32%, transparent);
  }
  @media (max-width: 720px) {
    .pane-pump { padding-right: 0; border-right: none; }
    .group.row > .pane { padding-block: var(--s-3); }
  }
  .pane-profile { flex: 1 1 320px; }
  .pane-profile .grid { display: flex; flex-wrap: wrap; gap: var(--s-4); }
  .pane-profile .field { flex: 0 1 140px; }
  .pane-profile .field .seg { align-self: flex-start; }
  .pump-fields { display: flex; gap: var(--s-4); }
  .pump-fields .seg button { padding: var(--s-2) var(--s-3); }
  .dur-field { grid-column: span 2; }
  .dur { display: flex; gap: var(--s-3); }
  .dur label {
    display: flex;
    align-items: baseline;
    gap: var(--s-2);
    flex: 1;
    min-width: 0;
  }
  .dur input {
    width: 100%;
    min-width: 0;
    text-align: right;
    -moz-appearance: textfield;
    appearance: textfield;
  }
  .dur input::-webkit-outer-spin-button,
  .dur input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }
  .dur label > span { font-size: 12px; color: var(--muted); }

  .preview {
    margin-top: var(--s-6);
    padding-top: var(--s-5);
    border-top: 1px solid color-mix(in srgb, var(--muted) 32%, transparent);
  }
  .pv-cap { font-size: 12px; color: var(--muted); margin-top: var(--s-2); }
  .muted { color: var(--muted); }
  .foot { margin-top: var(--s-6); }

  .fb {
    margin-top: var(--s-5);
    border: 1px solid var(--line);
    border-radius: var(--radius-ctl);
    padding: var(--s-3) var(--s-4);
    background: var(--surface-sunken);
  }
  .fb summary { cursor: pointer; font-weight: 600; font-size: 13px; }
  .fb[open] summary { margin-bottom: var(--s-4); }
  .fb-eq { font-size: 12px; color: var(--muted); margin: 0 0 var(--s-4); }
  .fb-out {
    font-size: 13px;
    margin: var(--s-4) 0;
    padding: var(--s-3);
    background: var(--surface);
    border-radius: var(--radius-ctl);
  }
  .fb-out .dim { color: var(--muted); }
</style>
