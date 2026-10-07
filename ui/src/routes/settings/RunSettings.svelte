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
    <label class="field check">
      <input type="checkbox" bind:checked={prompt} />
      <span>Ask before resuming an interrupted run</span>
    </label>

    <label class="field"><span>Calibration burst grace (minutes)</span>
      <input type="number" min="0" bind:value={graceMinutes} />
    </label>

    <p class="field-note">
      {prompt
        ? 'Ticked: after a restart, an interrupted run waits for someone to choose Resume, Finish or Abort.'
        : 'Unticked: a feeding run interrupted by a power cut, a crash or a reboot resumes by itself as soon as the pump answers, at the point its curve has reached. Nothing is asked.'}
      A calibration burst always waits for you, and can still be resumed this long past its end.
    </p>
  </div>

  {#snippet foot()}
    <button class="btn-primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
  {/snippet}
</SettingsCard>
