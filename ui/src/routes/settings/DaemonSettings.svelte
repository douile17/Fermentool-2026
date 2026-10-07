<script>
  import { post } from '../../lib/api.js';
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import SettingsCard from './SettingsCard.svelte';

  let { group } = $props();

  let loaded = $state(false);
  let port = $state(8730);
  let logLevel = $state('info');
  let err = $state(null);
  let msg = $state(null);
  let saving = $state(false);

  $effect(() => {
    loadConfig()
      .then((c) => {
        port = c.port;
        logLevel = c.log.level;
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
        c.port = Number(port);
        c.log.level = logLevel;
      });
      msg = res.note ? `Saved: ${res.note}` : 'Saved.';
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }

  let stopErr = $state(null);
  let stopped = $state(false);
  async function shutdown() {
    stopErr = null;
    try {
      await post('/api/shutdown');
      stopped = true;
    } catch (e) {
      stopErr = e.message;
    }
  }
</script>

<SettingsCard {group} title="Daemon" {err} {msg} loading={!loaded}>
  <div class="grid">
    <label class="field"><span>API port (restart to apply; the desktop app expects 8730)</span>
      <input type="number" bind:value={port} />
    </label>

    <label class="field"><span>Log level</span>
      <select bind:value={logLevel}>
        {#each ['error', 'warn', 'info', 'debug', 'trace'] as l}<option value={l}>{l}</option>{/each}
      </select>
    </label>
  </div>

  {#snippet foot()}
    <button class="btn-primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
  {/snippet}
</SettingsCard>

<SettingsCard
  group="Danger zone"
  title="Stop the daemon"
  intro="Stops the control loop and the API. A running pump keeps its last commanded speed; restart the daemon to resume the run."
  err={stopErr}
  msg={stopped ? 'Daemon stopped. This page is now offline.' : null}
>
  <button class="btn-danger" disabled={stopped} onclick={shutdown}>Shut down daemon</button>
</SettingsCard>
