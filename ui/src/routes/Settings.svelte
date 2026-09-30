<script>
  import { get, put, post } from '../lib/api.js';

  let cfg = $state(null);
  let err = $state(null);
  let msg = $state(null);
  let saving = $state(false);
  let stopped = $state(false);

  $effect(() => {
    get('/api/config')
      .then((c) => (cfg = c))
      .catch((e) => (err = e.message));
  });

  // Fields this page owns. serial.path and pump.address live in the connection
  // bar and may have changed there since this page loaded, merge our fields
  // onto the current server config instead of round-tripping a stale snapshot.
  async function save() {
    saving = true;
    err = null;
    msg = null;
    try {
      const fresh = await get('/api/config');
      fresh.port = cfg.port;
      fresh.serial.baud = cfg.serial.baud;
      fresh.serial.allow_simulator = cfg.serial.allow_simulator;
      fresh.resume.grace_minutes = cfg.resume.grace_minutes;
      fresh.resume.prompt = cfg.resume.prompt;
      fresh.log.level = cfg.log.level;
      const r = await put('/api/config', fresh);
      cfg = fresh;
      msg = r.note ? `Saved: ${r.note}` : 'Saved.';
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }

  // Balance: its own card and Save, applied live by the daemon (no restart).
  let ports = $state([]);
  let scalePort = $state('');
  let scaleCustom = $state('');
  let scaleBaud = $state(9600);
  let scaleDensity = $state(1.0);
  let scaleErr = $state(null);
  let scaleMsg = $state(null);
  let scaleSaving = $state(false);
  let scaleLoaded = false;

  function rescanPorts() {
    get('/api/serial/ports')
      .then((r) => (ports = r.ports ?? []))
      .catch((e) => (scaleErr = e.message));
  }
  $effect(() => rescanPorts());

  // Seed the balance fields once, from the first config load.
  $effect(() => {
    if (!cfg || scaleLoaded) return;
    scaleLoaded = true;
    const p = cfg.scale?.path?.trim() ?? '';
    scalePort = p;
    scaleCustom = p;
    scaleBaud = cfg.scale?.baud ?? 9600;
    scaleDensity = cfg.scale?.density_g_per_ml ?? 1.0;
  });

  const knownPort = $derived(scalePort === '' || ports.some((p) => p.name === scalePort));
  const pumpPort = $derived(cfg?.serial?.path ?? '');

  async function saveScale() {
    scaleSaving = true;
    scaleErr = null;
    scaleMsg = null;
    try {
      const path = scalePort === '__custom' ? scaleCustom.trim() : scalePort;
      if (path !== '' && path === pumpPort) throw new Error(`${path} is the pump's port, pick the balance's port`);
      const density = Number(scaleDensity);
      if (!(density > 0)) throw new Error('Liquid density must be a positive number (g/mL)');
      const fresh = await get('/api/config');
      fresh.scale = { path, baud: Number(scaleBaud), density_g_per_ml: density };
      const r = await put('/api/config', fresh);
      cfg.scale = fresh.scale;
      if (path === '') scaleMsg = 'Saved: no balance.';
      else if (r.scale_connected === false)
        scaleMsg = `Saved, but the balance does not answer on ${path} yet. Check the port, the cable and the baud; Fermentool keeps retrying.`;
      else scaleMsg = `Saved: balance on ${path}.`;
    } catch (e) {
      scaleErr = e.message;
    }
    scaleSaving = false;
  }

  async function shutdown() {
    try {
      await post('/api/shutdown');
      stopped = true;
    } catch (e) {
      err = e.message;
    }
  }
</script>

<section class="card">
  <div class="card-head"><div><div class="eyebrow">Settings</div><h2>Daemon configuration</h2></div></div>

  {#if err}<div class="err" style="margin-bottom:16px">{err}</div>{/if}
  {#if msg}<div class="ok" style="margin-bottom:16px">{msg}</div>{/if}

  {#if !cfg}
    <p class="muted">Loading…</p>
  {:else}
    <div class="grid">
      <label class="field"><span>API port (restart to apply)</span>
        <input type="number" bind:value={cfg.port} />
      </label>

      <p class="field-note">
        The serial port and pump MODBUS address are set in the connection bar at the top of
        the screen. Baud below is applied the next time you click Connect there.
      </p>

      <label class="field"><span>Baud</span>
        <select bind:value={cfg.serial.baud}>
          {#each [1200, 2400, 4800, 9600] as b}<option value={b}>{b}</option>{/each}
        </select>
      </label>

      <label class="field check">
        <input type="checkbox" bind:checked={cfg.serial.allow_simulator} />
        <span>Allow runs on the pump simulator (bench testing, no real pump)</span>
      </label>

      <label class="field"><span>Resume grace (minutes)</span>
        <input type="number" min="0" bind:value={cfg.resume.grace_minutes} />
      </label>

      <label class="field"><span>Log level</span>
        <select bind:value={cfg.log.level}>
          {#each ['error', 'warn', 'info', 'debug', 'trace'] as l}<option value={l}>{l}</option>{/each}
        </select>
      </label>

      <label class="field check">
        <input type="checkbox" bind:checked={cfg.resume.prompt} />
        <span>Ask before resuming an interrupted run</span>
      </label>
    </div>

    <div class="foot">
      <button class="btn-primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
    </div>
  {/if}
</section>

<section class="card">
  <div class="card-head"><div><div class="eyebrow">Settings</div><h2>Balance</h2></div></div>

  {#if scaleErr}<div class="err" style="margin-bottom:16px">{scaleErr}</div>{/if}
  {#if scaleMsg}<div class="ok" style="margin-bottom:16px">{scaleMsg}</div>{/if}

  {#if !cfg}
    <p class="muted">Loading…</p>
  {:else}
    <p class="field-note" style="margin-bottom:16px">
      The balance under the feed bottle (Ohaus Ranger, MT-SICS) that the gravimetric trim reads.
      Applied as soon as you save, no restart. Its live weight then shows at the bottom of the
      sidebar.
    </p>
    <div class="grid">
      <label class="field"><span>Port</span>
        <select bind:value={scalePort}>
          <option value="">No balance</option>
          {#each ports as p}
            <option value={p.name} disabled={p.name === pumpPort}>
              {p.name}{p.product ? ` · ${p.product}` : ''}{p.name === pumpPort ? ' (pump)' : ''}
            </option>
          {/each}
          {#if !knownPort && scalePort !== '__custom'}
            <option value={scalePort}>{scalePort} (not detected)</option>
          {/if}
          <option value="__custom">Other port…</option>
        </select>
      </label>

      {#if scalePort === '__custom'}
        <label class="field"><span>Port name</span>
          <input type="text" placeholder="COM5" bind:value={scaleCustom} />
        </label>
      {/if}

      <label class="field"><span>Baud</span>
        <select bind:value={scaleBaud}>
          {#each [1200, 2400, 4800, 9600, 19200, 38400] as b}<option value={b}>{b}</option>{/each}
        </select>
      </label>

      <label class="field"><span>Liquid density (g/mL)</span>
        <input type="number" step="0.01" min="0.5" max="2" bind:value={scaleDensity} />
      </label>

      <p class="field-note">
        Baud must match the balance's own Communications menu (9600 by default on the Ranger 7000).
        Density converts the weighed grams to mL: water and dilute feeds ≈ 1.00.
      </p>
    </div>

    <div class="foot">
      <button class="btn-primary" disabled={scaleSaving} onclick={saveScale}>
        {scaleSaving ? 'Connecting…' : 'Save balance'}
      </button>
      <button class="btn-ghost" onclick={rescanPorts}>Rescan ports</button>
    </div>
  {/if}
</section>

<section class="card">
  <div class="card-head"><div><div class="eyebrow">Danger zone</div><h2>Stop the daemon</h2></div></div>
  <p class="muted">
    Stops the control loop and the API. A running pump keeps its last commanded speed;
    restart the daemon to resume the run.
  </p>
  <button class="btn-danger" onclick={shutdown}>Shut down daemon</button>
  {#if stopped}
    <div class="ok" style="margin-top:16px">Daemon stopped. This page is now offline.</div>
  {/if}
</section>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: var(--s-4);
  }
  .field.check { flex-direction: row; align-items: center; gap: var(--s-2); }
  .field.check span { color: var(--ink); font-size: 13px; }
  .field-note {
    grid-column: 1 / -1;
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }
  .foot { margin-top: var(--s-6); }
  .muted { color: var(--muted); }
  .ok {
    color: var(--green-600);
    background: color-mix(in srgb, var(--green-500) 12%, var(--surface));
    border-radius: var(--radius-ctl);
    padding: var(--s-2) var(--s-3);
    font-size: 13px;
  }
</style>
