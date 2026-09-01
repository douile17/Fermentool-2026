<script>
  import { app } from '../lib/state.svelte.js';
  import { get, post } from '../lib/api.js';
  import { num, RPM_LIMITS } from '../lib/fmt.js';
  import Chart from '../components/Chart.svelte';

  const unit = 'rpm';

  let f = $state({
    name: '',
    direction: 'cw',
    duration_h: 24,
    tick_interval_s: 10,
    kind: 'linear',
    mode: 'endpoints',
    start: 5,
    end: 50,
    value: 10, // constant
    mu_per_hour: 0.15,
    steepness: 8,
    clamp_min: null,
    clamp_max: null,
  });

  let pumpAddr = $state(1);
  $effect(() => {
    get('/api/config')
      .then((c) => (pumpAddr = c.pump.address))
      .catch(() => {});
  });

  const durationS = $derived(Math.round(f.duration_h * 3600));

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
      clamp_min: f.clamp_min != null && f.clamp_min !== '' ? Number(f.clamp_min) : RPM_LIMITS.min,
      clamp_max: f.clamp_max != null && f.clamp_max !== '' ? Number(f.clamp_max) : RPM_LIMITS.max,
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
        control_var: 'rpm',
        direction: f.direction,
        tick_interval_s: Number(f.tick_interval_s),
        pump_addr: pumpAddr,
        curve: curveSpec(),
      });
      app.route = 'overview';
    } catch (e) {
      startErr = e.message;
      starting = false;
    }
  }

  const busy = $derived(app.status?.active != null);
</script>

<section class="card">
  <div class="card-head">
    <div><div class="eyebrow">New run</div><h2>Build a time profile</h2></div>
  </div>

  {#if busy}
    <div class="err" style="margin-bottom:16px">A run is already active — stop it from Overview first.</div>
  {/if}

  <p class="lede">
    The pump is driven directly in <b>rpm</b>. Convert to ml/min with a
    Fermentool calibration (coming soon).
  </p>

  <div class="grid">
    <label class="field"><span>Run name</span>
      <input type="text" bind:value={f.name} placeholder="ferment-A2" />
    </label>

    <label class="field"><span>Duration (hours)</span>
      <input type="number" min="0.1" step="0.5" bind:value={f.duration_h} />
    </label>

    <div class="field"><span>Direction</span>
      <div class="seg">
        <button class:on={f.direction === 'cw'} onclick={() => (f.direction = 'cw')}>clockwise</button>
        <button class:on={f.direction === 'ccw'} onclick={() => (f.direction = 'ccw')}>counter</button>
      </div>
    </div>

    <label class="field"><span>Curve</span>
      <select bind:value={f.kind}>
        <option value="linear">Linear</option>
        <option value="exponential">Exponential (fed-batch)</option>
        <option value="sigmoid">Sigmoid (logistic)</option>
        <option value="constant">Constant</option>
      </select>
    </label>

    <label class="field"><span>Tick every (seconds)</span>
      <input type="number" min="1" max="300" bind:value={f.tick_interval_s} />
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

    <label class="field"><span>Clamp min ({unit})</span>
      <input type="number" step="0.1" bind:value={f.clamp_min} placeholder={String(RPM_LIMITS.min)} />
    </label>
    <label class="field"><span>Clamp max ({unit})</span>
      <input type="number" step="0.1" bind:value={f.clamp_max} placeholder={String(RPM_LIMITS.max)} />
    </label>
  </div>

  <div class="preview">
    <div class="eyebrow" style="margin-bottom:8px">Preview</div>
    {#if previewErr}
      <div class="err">{previewErr}</div>
    {:else if preview.length}
      <Chart planned={preview} actual={[]} nowS={null} {durationS} {unit} />
      <div class="pv-cap mono">
        start {num(preview[0][1])} {unit} · end {num(preview[preview.length - 1][1])} {unit} · {f.duration_h} h
      </div>
    {:else}
      <p class="muted">Adjust the fields to see the curve.</p>
    {/if}
  </div>

  {#if startErr}<div class="err" style="margin-top:16px">{startErr}</div>{/if}

  <div class="foot">
    <button class="btn-primary" disabled={starting || busy || !!previewErr} onclick={start}>
      {starting ? 'Starting…' : 'Start run'}
    </button>
  </div>
</section>

<style>
  .lede { color: var(--muted); margin: 0 0 var(--s-5); max-width: 60ch; }
  .lede b { color: var(--ink); }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: var(--s-4);
  }
  .preview {
    margin-top: var(--s-6);
    padding-top: var(--s-5);
    border-top: 1px solid var(--line-soft);
  }
  .pv-cap { font-size: 12px; color: var(--muted); margin-top: var(--s-2); }
  .muted { color: var(--muted); }
  .foot { margin-top: var(--s-6); }
</style>
