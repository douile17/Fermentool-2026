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
  // bar and may have changed there since this page loaded - merge our fields
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
        <span>Allow runs on the pump simulator (bench testing - no real pump)</span>
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
