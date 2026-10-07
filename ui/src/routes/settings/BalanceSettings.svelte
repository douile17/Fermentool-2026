<script>
  import { get } from '../../lib/api.js';
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import SettingsCard from './SettingsCard.svelte';

  let { group } = $props();

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
        scaleCustom = p;
        scaleBaud = c.scale?.baud ?? 9600;
        scaleDensity = c.scale?.density_g_per_ml ?? 1.0;
        scalePosition = c.scale?.position ?? 'feed';
        trimLimit = c.scale?.trim_limit_pct ?? 25;
        loaded = true;
      })
      .catch((e) => (err = e.message));
  });

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
      if (path === '') msg = 'Saved: no balance.';
      else if (res.scale_connected === false)
        msg = `Saved, but the balance does not answer on ${path} yet. Check the port, the cable and the baud; Fermentool keeps retrying.`;
      else msg = `Saved: balance on ${path}.`;
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }
</script>

<SettingsCard
  {group}
  title="Balance"
  intro="The balance (Ohaus Ranger, MT-SICS) that the gravimetric trim reads, under the feed bottle or under the receiving vessel. Applied as soon as you save, no restart. Its live weight then shows at the bottom of the sidebar."
  {err}
  {msg}
  loading={!loaded}
>
  <div class="grid">
    <label class="field"><span>Port</span>
      <select bind:value={scalePort}>
        <option value="">No balance</option>
        {#each ports as p}
          <option value={p.name} disabled={p.name === pumpPort}>
            {p.name}{p.product ? ` | ${p.product}` : ''}{p.name === pumpPort ? ' (pump)' : ''}
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

    <label class="field"><span>Balance weighs</span>
      <select bind:value={scalePosition}>
        <option value="feed">the feed bottle (weight falls)</option>
        <option value="receiver">the receiving vessel (weight rises)</option>
      </select>
    </label>

    <label class="field"><span>Correction limit (± %)</span>
      <input type="number" step="1" min="5" max="100" bind:value={trimLimit} />
    </label>
    <p class="field-note">
      How far the balance correction may push the pump before it alarms instead
      (±{Number(trimLimit) || 25} %: factor ×{(1 / (1 + (Number(trimLimit) || 25) / 100)).toFixed(2)} to
      ×{(1 + (Number(trimLimit) || 25) / 100).toFixed(2)}). 25 by default. Wider tolerates a tube
      that delivers far from its calibration, but hides a slipping tube, a leak or a bad reading
      for longer. Can be changed during a run (the other balance settings cannot): widening it
      lifts an alarm raised by the old limit.
    </p>

    {#if scalePosition === 'receiver'}
      <p class="field-note">
        Under the receiving vessel, everything else added to it (pH base, antifoam) counts as
        delivered feed, and samples or evaporation count as missing. Under the feed bottle, only
        the pump is measured.
      </p>
    {/if}

    <p class="field-note">
      Baud must match the balance's own Communications menu (9600 by default on the Ranger 7000).
      Density converts the weighed grams to mL: water and dilute feeds ≈ 1.00.
    </p>
  </div>

  {#snippet foot()}
    <button class="btn-primary" disabled={saving} onclick={save}>
      {saving ? 'Connecting…' : 'Save balance'}
    </button>
    <button class="btn-ghost" onclick={rescanPorts}>Rescan ports</button>
  {/snippet}
</SettingsCard>
