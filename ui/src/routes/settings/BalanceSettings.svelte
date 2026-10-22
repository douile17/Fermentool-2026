<script>
  import { get, post } from '../../lib/api.js';
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import { app } from '../../lib/state.svelte.js';
  import { balanceLine } from '../../lib/balance.js';
  import Icon from '../../components/Icon.svelte';
  import LinkHeader from './LinkHeader.svelte';
  import SettingsCard from './SettingsCard.svelte';

  // `group` (passed to every section) is not shown here: the side menu says it.

  // Applied live by the daemon (no restart).
  let loaded = $state(false);
  let ports = $state([]);
  let pumpPort = $state('');
  let scalePort = $state('');
  let scaleCustom = $state('');
  let scaleBaud = $state(9600);
  let scaleDensity = $state(1.0);
  let scalePosition = $state('feed');
  let trimLimit = $state(25);
  let err = $state(null);
  let msg = $state(null);
  let saving = $state(false);
  // what the daemon has: Save waits for a change (UX audit, 2026-10-22)
  let savedForm = $state(null);
  const formKey = () => JSON.stringify([scalePort === '__custom' ? scaleCustom.trim() : scalePort, Number(scaleBaud), Number(scaleDensity), scalePosition, Number(trimLimit)]);
  const dirty = $derived(savedForm !== null && formKey() !== savedForm);

  function rescanPorts() {
    get('/api/serial/ports')
      .then((r) => (ports = r.ports ?? []))
      .catch((e) => (err = e.message));
  }
  $effect(() => rescanPorts());

  $effect(() => {
    loadConfig()
      .then((c) => {
        const p = c.scale?.path?.trim() ?? '';
        pumpPort = c.serial?.path ?? '';
        scalePort = p;
        savedPort = p;
        scaleCustom = p;
        scaleBaud = c.scale?.baud ?? 9600;
        scaleDensity = c.scale?.density_g_per_ml ?? 1.0;
        scalePosition = c.scale?.position ?? 'feed';
        trimLimit = c.scale?.trim_limit_pct ?? 25;
        savedForm = formKey();
        loaded = true;
      })
      .catch((e) => (err = e.message));
  });

  // the saved port's link, live from the status frame
  let savedPort = $state('');
  const line = $derived(balanceLine(app.status, savedPort));

  const formPath = $derived(scalePort === '__custom' ? scaleCustom.trim() : scalePort);

  // Connect: the port in the form; saved first if it changed (Save connects).
  async function connect() {
    if (formPath !== savedPort) return save();
    saving = true;
    err = null;
    msg = null;
    try {
      const r = await post('/api/scale/connect');
      msg = r.connected ? `Connected to ${savedPort}.` : `Connecting to ${savedPort}: Fermentool keeps retrying until the balance answers.`;
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }

  async function disconnect() {
    saving = true;
    err = null;
    msg = null;
    try {
      const r = await post('/api/scale/disconnect');
      msg = `${r.disconnected} closed: the weight is no longer read until you Connect.`;
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }

  const knownPort = $derived(scalePort === '' || ports.some((p) => p.name === scalePort));

  async function save() {
    saving = true;
    err = null;
    msg = null;
    try {
      const path = scalePort === '__custom' ? scaleCustom.trim() : scalePort;
      if (path !== '' && path === pumpPort) throw new Error(`${path} is the pump's port, pick the balance's port`);
      const density = Number(scaleDensity);
      if (!(density > 0)) throw new Error('Liquid density must be a positive number (g/mL)');
      const limit = Number(trimLimit);
      if (!(limit >= 5 && limit <= 100)) throw new Error('Correction limit must be between 5 and 100 %');
      const { res } = await patchConfig((c) => {
        c.scale = {
          path,
          baud: Number(scaleBaud),
          density_g_per_ml: density,
          position: scalePosition,
          trim_limit_pct: limit,
        };
      });
      savedPort = path;
      savedForm = formKey();
      if (path === '') msg = 'Saved: no balance.';
      else if (res.scale_connected === false) msg = `Saved. Connecting to ${path}: Fermentool keeps retrying until the balance answers.`;
      else msg = `Saved: balance on ${path}.`;
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }
</script>

<!-- Like Settings > Pumps: no eyebrow, no intro (the explanations belong in
     the technical documentation), the link's state first, then its port. -->
<SettingsCard title="Balance" {err} {msg} loading={!loaded}>
  <LinkHeader {line}>
    {#snippet action()}
      <!-- as in Pumps: Disconnect (red ink, no box) while it answers, Connect otherwise -->
      {#if app.status?.scale_connected}
        <button class="unplug" type="button" onclick={disconnect} disabled={saving}
          title="Close the port: the weight is no longer read until Connect"><Icon name="unplug" size={15} />Disconnect</button>
      {:else}
        <button class="btn-primary" type="button" onclick={connect} disabled={saving || !formPath}>Connect</button>
      {/if}
    {/snippet}
  </LinkHeader>
  <div class="conn">
    <select class="port" bind:value={scalePort} aria-label="Balance port">
      <option value="">No balance</option>
      {#each ports as p}
        <option value={p.name} disabled={p.name === pumpPort}>
          {p.name}{p.product ? ` | ${p.product}` : ''}{p.name === pumpPort ? ' (pumps)' : ''}
        </option>
      {/each}
      {#if !knownPort && scalePort !== '__custom'}
        <option value={scalePort}>{scalePort} (not detected)</option>
      {/if}
      <option value="__custom">Other port…</option>
    </select>
    {#if scalePort === '__custom'}
      <input class="custom" type="text" placeholder="COM5" bind:value={scaleCustom} aria-label="Port name" />
    {/if}
    <button class="iconbtn" type="button" title="Rescan the ports" aria-label="Rescan the ports" onclick={rescanPorts}>
      <Icon name="refresh-cw" size={16} />
    </button>
    <select bind:value={scaleBaud} aria-label="Baud">
      {#each [1200, 2400, 4800, 9600, 19200, 38400] as b}<option value={b}>{b} baud</option>{/each}
    </select>
  </div>

  <div class="grid three">
    <label class="field"><span>Liquid density, g/mL</span>
      <input type="number" step="0.01" min="0.5" max="2" bind:value={scaleDensity} />
    </label>
    <label class="field"><span>Balance weighs</span>
      <select bind:value={scalePosition}>
        <option value="feed">the feed bottle</option>
        <option value="receiver">the receiving vessel</option>
      </select>
    </label>
    <label class="field"><span>Correction limit, ± %</span>
      <input type="number" step="1" min="5" max="100" bind:value={trimLimit} />
      <small class="hint">×{(1 / (1 + (Number(trimLimit) || 25) / 100)).toFixed(2)} to ×{(1 + (Number(trimLimit) || 25) / 100).toFixed(2)}</small>
    </label>
  </div>
  {#if scalePosition === 'receiver'}
    <p class="field-note warn">Anything else added to the vessel (base, antifoam) counts as feed.</p>
  {/if}

  {#snippet foot()}
    <button class="btn-primary save" disabled={saving || !dirty} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
  {/snippet}
</SettingsCard>

<style>
  .conn { display: flex; gap: var(--s-2); align-items: center; flex-wrap: wrap; margin-bottom: var(--s-5); }
  /* app.css sizes every field to full width: here they share one line */
  .conn select, .conn input { width: auto; }
  .conn .port { flex: 1 1 260px; min-width: 0; }
  .conn .custom { width: 140px; }
  .iconbtn {
    width: 38px; align-self: stretch; padding: 0; display: grid; place-items: center; flex: none; cursor: pointer;
    border: 1px solid var(--line); border-radius: var(--radius-ctl); background: var(--surface); color: var(--muted);
  }
  .iconbtn:hover { color: var(--teal-700); border-color: var(--teal-700); }
  .iconbtn:focus-visible { outline: 2px solid var(--teal-600); outline-offset: 2px; }
  .grid.three { grid-template-columns: repeat(3, minmax(0, 1fr)); }
  @media (max-width: 640px) { .grid.three { grid-template-columns: minmax(0, 1fr); } }
  .hint { font-size: 12px; color: var(--muted); }
  .warn { color: var(--warn-ink); margin-top: var(--s-3); }
  .save { margin-left: auto; }
</style>
