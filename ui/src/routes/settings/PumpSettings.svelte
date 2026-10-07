<script>
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import SettingsCard from './SettingsCard.svelte';

  let { group } = $props();

  let loaded = $state(false);
  let baud = $state(9600);
  let allowSim = $state(false);
  let err = $state(null);
  let msg = $state(null);
  let saving = $state(false);

  $effect(() => {
    loadConfig()
      .then((c) => {
        baud = c.serial.baud;
        allowSim = c.serial.allow_simulator;
        loaded = true;
      })
      .catch((e) => (err = e.message));
  });

  async function save() {
    saving = true;
    err = null;
    msg = null;
    try {
      const { res } = await patchConfig((c) => {
        c.serial.baud = Number(baud);
        c.serial.allow_simulator = allowSim;
      });
      msg = res.note ? `Saved: ${res.note}` : 'Saved.';
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }
</script>

<SettingsCard
  {group}
  title="Pump"
  intro="The serial port and pump MODBUS address are set in the connection bar at the top of the screen. Baud below is applied the next time you click Connect there."
  {err}
  {msg}
  loading={!loaded}
>
  <div class="grid">
    <label class="field"><span>Baud</span>
      <select bind:value={baud}>
        {#each [1200, 2400, 4800, 9600] as b}<option value={b}>{b}</option>{/each}
      </select>
    </label>

    <label class="field check">
      <input type="checkbox" bind:checked={allowSim} />
      <span>Allow runs on the pump simulator (bench testing, no real pump)</span>
    </label>
  </div>

  {#snippet foot()}
    <button class="btn-primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
  {/snippet}
</SettingsCard>
