<script>
  import { app } from '../../lib/state.svelte.js';
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import { sizeParts } from '../../lib/tubesize.js';
  import SettingsCard from './SettingsCard.svelte';

  let { group } = $props();

  let loaded = $state(false);
  let tubeSizes = $state('mm');
  let err = $state(null);
  let msg = $state(null);
  let saving = $state(false);

  $effect(() => {
    loadConfig()
      .then((c) => {
        tubeSizes = c.display?.tube_sizes === 'in' ? 'in' : 'mm';
        loaded = true;
      })
      .catch((e) => (err = e.message));
  });

  // What a 1/32" x 1/16" tube looks like with each choice.
  const sample = $derived(sizeParts(0.8, 1.6, tubeSizes));

  async function save() {
    saving = true;
    err = null;
    msg = null;
    try {
      await patchConfig((c) => {
        c.display = { ...(c.display ?? {}), tube_sizes: tubeSizes };
      });
      app.tubeSizes = tubeSizes;
      msg = 'Saved.';
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }
</script>

<SettingsCard
  {group}
  title="Display"
  intro="How sizes are shown, the same for everyone on this PC."
  {err}
  {msg}
  loading={!loaded}
>
  <div class="field">
    <span>Tube sizes in</span>
    <div class="seg" role="radiogroup" aria-label="Tube sizes in">
      <button role="radio" aria-checked={tubeSizes === 'mm'} class:on={tubeSizes === 'mm'} onclick={() => (tubeSizes = 'mm')}>mm first</button>
      <button role="radio" aria-checked={tubeSizes === 'in'} class:on={tubeSizes === 'in'} onclick={() => (tubeSizes = 'in')}>inches first</button>
    </div>
  </div>
  <p class="sample">
    A tube reads <span class="pri mono">{sample.pri}</span> <span class="sec mono">{sample.sec}</span>.
    The calibration's diameter entry starts in this unit too.
  </p>

  {#snippet foot()}
    <button class="btn-primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
  {/snippet}
</SettingsCard>

<style>
  .field { align-items: flex-start; }
  .sample { margin: var(--s-3) 0 0; font-size: 13px; color: var(--muted); }
  .pri { color: var(--ink); font-weight: 600; }
  .sec { font-size: 0.9em; }
</style>
