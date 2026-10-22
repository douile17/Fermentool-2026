<script>
  import { app } from '../../lib/state.svelte.js';
  import { post } from '../../lib/api.js';
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import SettingsCard from './SettingsCard.svelte';

  // `group` (passed to every section) is not shown here: the side menu says it.

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
  let confirmStop = $state(false);
  const runName = $derived(app.status?.active?.name ?? null);
  async function shutdown() {
    stopErr = null;
    try {
      await post('/api/shutdown');
      stopped = true;
      confirmStop = false;
    } catch (e) {
      stopErr = e.message;
    }
  }
</script>

<SettingsCard title="Daemon" {err} {msg} loading={!loaded}>
  <div class="grid">
    <label class="field"><span>API port</span>
      <input type="number" bind:value={port} />
      <small class="hint">applies after a restart</small>
    </label>

    <label class="field"><span>Log level</span>
      <select bind:value={logLevel}>
        {#each ['error', 'warn', 'info', 'debug', 'trace'] as l}<option value={l}>{l}</option>{/each}
      </select>
    </label>
  </div>

  {#snippet foot()}
    <button class="btn-primary save" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
  {/snippet}
</SettingsCard>

<SettingsCard
  title="Stop the daemon"
  intro="The pump keeps its last speed until the daemon is started again."
  err={stopErr}
  msg={stopped ? 'Daemon stopped. This page is now offline.' : null}
>
  {#if confirmStop}
    <p class="field-note confirm-note">
      {runName
        ? `Run “${runName}” is in progress: its regulation stops, the pump stays at its last speed, uncorrected and unrecorded, until the daemon is started again.`
        : 'The page goes offline until the daemon is started again.'}
      Shut it down?
    </p>
    <div class="row">
      <button class="btn-danger" onclick={shutdown}>Shut down daemon</button>
      <button class="btn-ghost" onclick={() => (confirmStop = false)}>Cancel</button>
    </div>
  {:else}
    <!-- red outline, as Disconnect: the confirmation below is the filled red one -->
    <button class="btn-ghost stop" disabled={stopped} onclick={() => (confirmStop = true)}>Shut down daemon</button>
  {/if}
</SettingsCard>

<style>
  .hint { font-size: 12px; color: var(--muted); }
  .stop { color: var(--danger-ink); border-color: color-mix(in srgb, var(--danger) 55%, var(--surface)); }
  .stop:not(:disabled):hover { background: var(--danger-bg); border-color: var(--danger); }
  .confirm-note { margin-bottom: var(--s-3); color: var(--danger-ink); }
  .row { display: flex; gap: var(--s-3); flex-wrap: wrap; }
</style>
