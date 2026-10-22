<script>
  // The Overview's "Change rate" dialog for a running Constant run: the rate
  // now, the new one in the run's unit within the daemon's bounds, then
  // confirmed (a typing slip would change the culture).
  import { post } from '../lib/api.js';
  import { num, unitFor, digitsFor } from '../lib/fmt.js';
  import { boundsLine, checkRate, rateLabel, rateSteps } from '../lib/ratechange.js';
  import ErrorText from './ErrorText.svelte';

  /** @type {{ run: any, bounds: [number, number] | null, trimmed?: boolean, onclose: () => void, onchanged?: () => void }} */
  let { run, bounds, trimmed = false, onclose, onchanged = () => {} } = $props();

  const unit = $derived(unitFor(run.control_var));
  const digits = $derived(digitsFor(run.control_var));
  const steps = $derived(rateSteps(run));
  const current = $derived(steps.at(-1).value);

  let value = $state(null);
  let busy = $state(false);
  let err = $state(null);
  let input = $state();
  $effect(() => input?.focus());

  const problem = $derived(checkRate(value, bounds, current, run.control_var));
  // Said once something is typed, not before (DESIGN.md, forms).
  const typed = $derived(value != null && value !== '');

  async function apply() {
    if (problem || busy) return;
    busy = true;
    err = null;
    try {
      await post('/api/runs/active/rate', { value: Number(value) });
      onchanged();
      onclose();
    } catch (e) {
      err = { message: e.message, hint: e.hint ?? null };
    }
    busy = false;
  }
  function onKey(e) {
    if (e.key === 'Escape' && !busy) onclose();
  }
</script>

<svelte:window onkeydown={onKey} />

<button class="backdrop" aria-label="Cancel" onclick={() => !busy && onclose()}></button>
<div class="layer">
  <div class="modal card confirm" role="dialog" aria-modal="true" aria-labelledby="ft-rate">
    <div class="eyebrow">Change rate</div>
    <h2 id="ft-rate">New constant rate</h2>
    <p class="lede">
      Now <b class="mono">{num(current, digits)}</b> {unit}, {steps.length === 1 ? 'since the start' : `since ${rateLabel(steps.at(-1))}`}.
    </p>
    <form onsubmit={(e) => { e.preventDefault(); apply(); }}>
      <label class="field">
        <span>New rate, {unit}</span>
        <span class="in">
          <input bind:this={input} type="number" inputmode="decimal" min="0" step="any" bind:value
            class:need={typed && problem} aria-describedby="ft-rate-b" />
          <span class="u">{unit}</span>
        </span>
        <small id="ft-rate-b">{boundsLine(bounds, run.control_var)}</small>
        {#if typed && problem}<small class="bad">{problem}</small>{/if}
      </label>
      <p class="lede">
        From now until the end of the run, or the next change.{trimmed ? ' The balance correction carries on.' : ''} Kept in the run's history.
      </p>
      {#if err}<ErrorText message={err.message} hint={err.hint} />{/if}
      <div class="modal-foot">
        <button type="submit" class="btn-primary" disabled={busy || !!problem}>
          {busy ? 'Changing…' : problem ? 'Change rate' : `Change to ${num(Number(value), digits)} ${unit}`}
        </button>
        <button type="button" class="btn-ghost" disabled={busy} onclick={onclose}>Cancel</button>
      </div>
    </form>
  </div>
</div>

<style>
  .backdrop {
    position: fixed; inset: 0; border: none; padding: 0;
    background: rgba(18, 41, 46, 0.28); cursor: default; z-index: 60;
  }
  .layer {
    position: fixed; inset: 0; display: grid; place-items: center;
    padding: var(--s-5); z-index: 61; pointer-events: none;
  }
  .modal { box-shadow: var(--shadow-pop); pointer-events: auto; animation: pop 0.18s ease-out; }
  @keyframes pop {
    from { transform: scale(0.97); opacity: 0; }
    to { transform: scale(1); opacity: 1; }
  }
  @media (prefers-reduced-motion: reduce) { .modal { animation: none; } }
  .confirm { width: 100%; max-width: 440px; }
  .confirm h2 { font-size: 17px; margin: 6px 0 var(--s-3); }
  .lede { margin: 0 0 var(--s-4); color: var(--muted); font-size: 13px; }
  .lede b { color: var(--ink); font-weight: 600; }
  .field { margin-bottom: var(--s-4); }
  .in { display: flex; align-items: center; gap: var(--s-2); }
  .in input { width: 160px; font-family: var(--mono); font-size: 18px; }
  .in input.need { border-color: var(--measured); }
  .u { color: var(--muted); white-space: nowrap; }
  small { font-size: 12px; color: var(--muted); }
  small.bad { color: var(--warn-ink); }
  .modal-foot { display: flex; gap: var(--s-3); justify-content: flex-end; flex-wrap: wrap; margin-top: var(--s-5); }
  .modal-foot button { min-height: 40px; }
</style>
