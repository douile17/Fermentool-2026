<script>
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import SettingsCard from './SettingsCard.svelte';

  // `group` (passed to every section) is not shown here: the side menu says it.
  // The calibration measurement grace (`resume.grace_minutes`) stays in
  // config.toml only: whoever calibrates is at the bench and redoes a
  // measurement rather than resuming it (2026-10-22).

  let loaded = $state(false);
  let prompt = $state(true);
  let err = $state(null);
  let msg = $state(null);
  let saving = $state(false);

  $effect(() => {
    loadConfig()
      .then((c) => {
        prompt = c.resume.prompt;
        loaded = true;
      })
      .catch((e) => (err = e.message));
  });

  // A switch: applied on the click, no Save (as Display).
  async function save() {
    saving = true;
    err = null;
    msg = null;
    try {
      const { res } = await patchConfig((c) => {
        c.resume.prompt = prompt;
      });
      msg = res.note ? `Saved: ${res.note}` : 'Saved.';
    } catch (e) {
      err = e.message;
      prompt = !prompt;
    }
    saving = false;
  }
</script>

<SettingsCard
  title="Crash resume"
  {err}
  {msg}
  loading={!loaded}
>
  <!-- one setting a row: what it is on the left, its control on the right -->
  <div class="rows">
    <label class="field check srow">
      <input type="checkbox" bind:checked={prompt} disabled={saving} onchange={save} />
      <span class="what">
        <b>Ask before resuming an interrupted run</b>
        <small>{prompt ? 'After a restart, the run waits for Resume, Finish or Abort.' : 'After a restart, the run resumes by itself where its curve is.'}</small>
      </span>
    </label>
  </div>
</SettingsCard>

<style>
  .rows { display: flex; flex-direction: column; }
  .srow { display: flex; align-items: center; justify-content: space-between; gap: var(--s-5); padding: var(--s-3) 0; }
  /* the next option goes in another .srow: give it `.srow + .srow { border-top: 1px solid var(--line-soft); }` */
  .what { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .what b { font-weight: 500; font-size: 14px; color: var(--ink); }
  .what small { font-size: 12.5px; color: var(--muted); }
</style>
