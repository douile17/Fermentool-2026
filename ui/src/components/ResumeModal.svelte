<script>
  import { post } from '../lib/api.js';
  import { num, dur } from '../lib/fmt.js';
  import { unitFor, digitsFor } from '../lib/fmt.js';
  import { app } from '../lib/state.svelte.js';
  import { canStartRun } from '../lib/link.js';

  let { info, ondone } = $props();
  let busy = $state(false);
  let err = $state(null);

  const unit = $derived(unitFor(info.control_var));
  const digits = $derived(digitsFor(info.control_var));
  // Resuming drives the pump, so it needs a live link, same gate as a new run.
  const blocked = $derived(!canStartRun(app.status));

  async function act(fn) {
    busy = true;
    err = null;
    try {
      await fn();
      ondone();
    } catch (e) {
      err = e.message;
      busy = false;
    }
  }

  const resume = () => act(() => post('/api/recovery/resume'));
  const finish = () => act(() => post('/api/recovery/discard', { status: 'completed' }));
  const abort = () => act(() => post('/api/recovery/discard', { status: 'aborted' }));
</script>

<div class="backdrop">
  <div class="modal card" role="dialog" aria-modal="true" aria-labelledby="rt">
    <div class="eyebrow">Crash recovery</div>
    <h2 id="rt">Resume run “{info.name}”?</h2>
    <p>
      This run was interrupted. It has been running for
      <b class="mono">{dur(info.elapsed_s)}</b> of
      <b class="mono">{dur(info.duration_s)}</b>.
      {#if info.past_end}
        The curve already finished while the app was down, so you can only close it out.
      {:else}
        Resuming applies the profile value for the real elapsed time now
        (<b class="mono">{num(info.resume_target, digits)} {unit}</b>) and re-runs the full pump
        start sequence.
      {/if}
    </p>

    {#if err}<div class="err">{err}</div>{/if}
    {#if blocked && !info.past_end}
      <div class="err">
        The pump link is down, resuming isn't possible until it's back (or simulator runs are
        enabled in Settings). You can still finish or abort the run.
      </div>
    {/if}

    <div class="row">
      {#if !info.past_end}
        <button class="btn-primary" disabled={busy || blocked} onclick={resume}>
          Resume at {num(info.resume_target, digits)} {unit}
        </button>
      {/if}
      <button class="btn-ghost" disabled={busy} onclick={finish}>Finish run</button>
      <button class="btn-danger" disabled={busy} onclick={abort}>Abort</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(18, 41, 46, 0.28);
    display: grid;
    place-items: center;
    padding: var(--s-5);
    z-index: 50;
  }
  .modal { max-width: 480px; box-shadow: var(--shadow-pop); }
  .modal h2 { font-size: 18px; margin: 6px 0 var(--s-3); }
  .modal p { margin: 0 0 var(--s-4); color: var(--muted); }
  .modal p b { color: var(--ink); font-weight: 600; }
  .row { display: flex; gap: var(--s-3); flex-wrap: wrap; margin-top: var(--s-4); }
</style>
