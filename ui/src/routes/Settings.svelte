<script>
  import { get, put, post } from '../lib/api.js';
  import { app } from '../lib/state.svelte.js';

  let cfg = $state(null);
  let ports = $state([]);
  let portErr = $state(null);
  let err = $state(null);
  let msg = $state(null);
  let saving = $state(false);
  let reconnecting = $state(false);
  let stopped = $state(false);

  $effect(() => {
    get('/api/config').then((c) => (cfg = c)).catch((e) => (err = e.message));
    rescan();
  });

  function rescan() {
    portErr = null;
    get('/api/serial/ports')
      .then((r) => (ports = r.ports ?? []))
      .catch((e) => (portErr = e.message));
  }

  async function reconnect() {
    reconnecting = true;
    err = null;
    msg = null;
    try {
      const r = await post('/api/serial/reconnect', {
        path: cfg.serial.path,
        baud: cfg.serial.baud,
      });
      msg = r.connected ? `Connected — ${r.connected}` : 'Reconnected.';
    } catch (e) {
      err = e.message;
    }
    reconnecting = false;
  }

  async function save() {
    saving = true;
    err = null;
    msg = null;
    try {
      const r = await put('/api/config', cfg);
      msg = r.note ? `Saved — ${r.note}` : 'Saved.';
    } catch (e) {
      err = e.message;
    }
    saving = false;
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

      <label class="field"><span>Serial port ("sim" for the simulator)</span>
        <input type="text" list="ports" bind:value={cfg.serial.path} />
        <datalist id="ports">
          {#each ports as p}<option value={p.name}>{p.name} — {p.product ?? p.kind}</option>{/each}
          <option value="sim">sim</option>
        </datalist>
        <div class="port-row">
          <button type="button" class="btn-ghost" onclick={rescan}>Rescan</button>
          <button type="button" class="btn-ghost" disabled={reconnecting} onclick={reconnect}>
            {reconnecting ? 'Connecting…' : 'Connect now'}
          </button>
          <span class="port-now">Connected to: <b>{app.status?.transport ?? '—'}</b></span>
        </div>
        {#if portErr}<div class="err" style="margin-top:8px">{portErr}</div>{/if}
      </label>

      <label class="field"><span>Baud</span>
        <select bind:value={cfg.serial.baud}>
          {#each [1200, 2400, 4800, 9600] as b}<option value={b}>{b}</option>{/each}
        </select>
      </label>

      <label class="field"><span>Pump address</span>
        <input type="number" min="1" max="247" bind:value={cfg.pump.address} />
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
  .port-row {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    margin-top: var(--s-2);
    flex-wrap: wrap;
  }
  .port-now { color: var(--muted); font-size: 13px; }
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
