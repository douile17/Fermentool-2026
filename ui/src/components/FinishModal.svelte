<script>
  import { post } from '../lib/api.js';
  import { num, unitFor, digitsFor } from '../lib/fmt.js';

  /** @type {{ hold: { run_id:number, name:string, control_var:string, value:number, finished_at:string }, ondismiss: () => void }} */
  let { hold, ondismiss } = $props();

  const unit = $derived(unitFor(hold.control_var));
  const digits = $derived(digitsFor(hold.control_var));

  let busy = $state(false);
  let err = $state(null);

  async function stopPump() {
    busy = true;
    err = null;
    try {
      await post('/api/pump/stop');
      ondismiss();
    } catch (e) {
      err = e.message;
      busy = false;
    }
  }

  function onKey(e) {
    if (e.key === 'Escape') ondismiss();
  }
</script>

<svelte:window on:keydown={onKey} />

<button class="backdrop" aria-label="Dismiss" onclick={ondismiss}></button>

<div class="layer">
  <div class="modal card" role="dialog" aria-modal="true" aria-labelledby="ft-done">
    <div class="mark" aria-hidden="true">
      <svg viewBox="0 0 52 52">
        <circle class="ring" cx="26" cy="26" r="24" />
        <path class="tick" d="M15 27 l8 8 l15 -17" />
      </svg>
    </div>

    <div class="eyebrow">Run complete</div>
    <h2 id="ft-done">“{hold.name}” finished</h2>

    <p class="lede">
      The pump is <b>still running</b> at the profile's final rate
      (<b class="mono">{num(hold.value, digits)}&nbsp;{unit}</b>) and holds there
      until you stop it.
    </p>

    {#if err}<div class="err">{err}</div>{/if}

    <div class="row">
      <button class="btn-danger" disabled={busy} onclick={stopPump}>
        {busy ? 'Stopping…' : 'Stop pump'}
      </button>
      <button class="btn-ghost" disabled={busy} onclick={ondismiss}>Keep it running</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    border: none;
    padding: 0;
    background: rgba(18, 41, 46, 0.28);
    cursor: default;
    z-index: 60;
  }
  .layer {
    position: fixed;
    inset: 0;
    display: grid;
    place-items: center;
    padding: var(--s-5);
    z-index: 61;
    pointer-events: none;
  }
  .modal {
    max-width: 440px;
    box-shadow: var(--shadow-pop);
    text-align: center;
    pointer-events: auto;
    animation: pop 0.18s ease-out;
  }
  @keyframes pop {
    from { transform: scale(0.96); opacity: 0; }
    to { transform: scale(1); opacity: 1; }
  }
  .modal h2 { font-size: 19px; margin: 8px 0 var(--s-3); }
  .lede { margin: 0; color: var(--muted); }
  .lede b { color: var(--ink); font-weight: 600; }

  .mark {
    width: 64px;
    height: 64px;
    margin: 0 auto var(--s-4);
  }
  .mark svg { width: 100%; height: 100%; overflow: visible; }
  .mark .ring {
    fill: color-mix(in srgb, var(--green-500) 14%, var(--surface));
    stroke: var(--green-500);
    stroke-width: 2;
  }
  .mark .tick {
    fill: none;
    stroke: var(--green-600);
    stroke-width: 4;
    stroke-linecap: round;
    stroke-linejoin: round;
    stroke-dasharray: 40;
    stroke-dashoffset: 0; /* resting = drawn; the animation is just the entrance */
    animation: draw 0.45s 0.1s ease-out;
  }
  @keyframes draw {
    from { stroke-dashoffset: 40; }
    to { stroke-dashoffset: 0; }
  }

  .row {
    display: flex;
    gap: var(--s-3);
    justify-content: center;
    flex-wrap: wrap;
    margin-top: var(--s-5);
  }

  .err { margin-top: var(--s-3); }

  @media (prefers-reduced-motion: reduce) {
    .mark .tick { animation: none; }
    .modal { animation: none; }
  }
</style>
