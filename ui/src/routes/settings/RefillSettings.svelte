<script>
  import { post } from '../../lib/api.js';
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import { app } from '../../lib/state.svelte.js';
  import { directionLabel, refillProblem, refillView } from '../../lib/refill.js';
  import { radioKeys, rovingTab } from '../../lib/radio.js';
  import SettingsCard from './SettingsCard.svelte';

  let { group } = $props();

  // Applied live by the daemon (no restart).
  let loaded = $state(false);
  let address = $state(2);
  let speed = $state(350);
  let direction = $state('cw');
  let feedAddress = $state(1);
  // What the daemon has: a Test runs that, so it waits for a save.
  let saved = $state(null);
  let err = $state(null);
  let msg = $state(null);
  let busy = $state(false);

  function take(c) {
    feedAddress = c.pump?.address ?? 1;
    const r = c.refill ?? {};
    address = r.pump_address ?? 2;
    speed = r.speed_rpm ?? 350;
    direction = r.direction ?? 'cw';
    saved = { address: Number(address), speed: Number(speed), direction };
  }

  $effect(() => {
    loadConfig()
      .then((c) => {
        take(c);
        loaded = true;
      })
      .catch((e) => (err = e.message));
  });

  const dirty = $derived(
    !!saved &&
      (saved.address !== Number(address) ||
        saved.speed !== Number(speed) ||
        saved.direction !== direction),
  );
  const view = $derived(refillView(app.status?.refill));
  const testing = $derived(!!app.status?.refill?.testing);
  const running = $derived(!!app.status?.active);

  async function save() {
    const problem = refillProblem({ address, feedAddress, speedRpm: speed });
    if (problem) {
      err = problem;
      msg = null;
      return false;
    }
    busy = true;
    err = null;
    msg = null;
    try {
      const { cfg } = await patchConfig((c) => {
        c.refill = {
          ...c.refill,
          pump_address: Number(address),
          speed_rpm: Number(speed),
          direction,
        };
      });
      take(cfg);
      busy = false;
      return true;
    } catch (e) {
      err = e.message;
      busy = false;
      return false;
    }
  }

  async function saveClicked() {
    if (await save()) msg = `Saved: refill pump at address ${saved.address}.`;
  }

  async function test() {
    busy = true;
    err = null;
    msg = null;
    try {
      const r = await post('/api/refill/test');
      msg = `Running ${r.seconds} s at ${saved.speed} rpm, ${directionLabel(saved.direction)}: the liquid must go from the reservoir into the weighed bottle. If it goes the other way, pick the other direction, save and test again.`;
    } catch (e) {
      err = e.message;
    }
    busy = false;
  }
</script>

<SettingsCard
  {group}
  title="Refill pump"
  {err}
  {msg}
  loading={!loaded}
>
  <div class="grid">
    <label class="field"><span>MODBUS address</span>
      <input type="number" min="1" max="247" step="1" bind:value={address} />
    </label>

    <label class="field"><span>Speed (rpm)</span>
      <input type="number" min="0.1" max="350" step="1" bind:value={speed} />
    </label>

    <div class="field"><span>Direction</span>
      <div class="seg" role="radiogroup" aria-label="Refill pump direction" tabindex="-1" onkeydown={radioKeys}>
        <button role="radio" aria-checked={direction === 'cw'} tabindex={rovingTab(direction === 'cw', false)}
          class:on={direction === 'cw'} title="Clockwise" onclick={() => (direction = 'cw')}>↻ cw</button>
        <button role="radio" aria-checked={direction === 'ccw'} tabindex={rovingTab(direction === 'ccw', false)}
          class:on={direction === 'ccw'} title="Counter-clockwise" onclick={() => (direction = 'ccw')}>↺ ccw</button>
      </div>
    </div>

    {#if view}
      <p class="link {view.tone}" title={view.title}>
        <span class="dot" aria-hidden="true"></span>{view.label}
      </p>
    {/if}

  </div>

  {#snippet foot()}
    <button class="btn-primary" disabled={busy || !dirty} onclick={saveClicked}>
      {busy ? 'Saving…' : 'Save refill pump'}
    </button>
    <button
      class="btn-ghost"
      disabled={busy || dirty || testing || running}
      title={dirty ? 'Save first: the test runs what is saved' : running ? 'Not during a run' : undefined}
      onclick={test}
    >
      {testing ? 'Testing…' : 'Test (3 s)'}
    </button>
  {/snippet}
</SettingsCard>

<style>
  .seg { align-self: start; }

  .link {
    grid-column: 1 / -1;
    margin: 0;
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
    font-size: 13px;
  }
  .link .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--muted); flex: none; }
  .link.ok { color: var(--green-600); }
  .link.ok .dot { background: var(--green-500); }
  .link.warn { color: var(--warn-ink); }
  .link.warn .dot { background: #d68b45; }
  .link.bad { color: var(--danger-ink); }
  .link.bad .dot { background: var(--danger); }
</style>
