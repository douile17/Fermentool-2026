<script>
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import SettingsCard from './SettingsCard.svelte';

  let { group } = $props();

  let loaded = $state(false);
  let graceMinutes = $state(0);
  let prompt = $state(true);
  let err = $state(null);
  let msg = $state(null);
  let saving = $state(false);

  $effect(() => {
    loadConfig()
      .then((c) => {
        graceMinutes = c.resume.grace_minutes;
        prompt = c.resume.prompt;
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
        c.resume.grace_minutes = Number(graceMinutes);
        c.resume.prompt = prompt;
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
  title="Crash resume"
  intro="What happens when the daemon restarts with a run still open (power cut, crash, reboot)."
  {err}
  {msg}
  loading={!loaded}
>
  <div class="grid">
    <label class="field"><span>Resume grace (minutes)</span>
      <input type="number" min="0" bind:value={graceMinutes} />
    </label>

    <label class="field check">
      <input type="checkbox" bind:checked={prompt} />
      <span>Ask before resuming an interrupted run</span>
    </label>
  </div>

  {#snippet foot()}
    <button class="btn-primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
  {/snippet}
</SettingsCard>
