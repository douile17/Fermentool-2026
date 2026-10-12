<script>
  // "Leave the calibration?": asked when a page that holds work (see
  // app.leaveGuard) is left for another. Stay keeps everything; Leave clears
  // it first (the page's own `leave`), then goes where the click asked.
  /** @type {{ guard: { title: string, lines: string[], leave: () => Promise<boolean> }, onstay: () => void, ongo: () => void }} */
  let { guard, onstay, ongo } = $props();

  let busy = $state(false);
  let err = $state(null);

  async function leave() {
    busy = true;
    err = null;
    const ok = await guard.leave().catch((e) => ((err = e.message), false));
    busy = false;
    if (ok) ongo();
    else err ??= 'Could not clear it: the page stays as it is.';
  }

  function onKey(e) {
    if (e.key === 'Escape' && !busy) onstay();
  }
</script>

<svelte:window onkeydown={onKey} />

<button class="backdrop" aria-label="Stay" onclick={() => !busy && onstay()}></button>
<div class="layer">
  <div class="modal card" role="alertdialog" aria-modal="true" aria-labelledby="ft-leave">
    <h2 id="ft-leave">{guard.title}</h2>
    <ul>
      {#each guard.lines as l}<li>{l}</li>{/each}
    </ul>
    {#if err}<div class="err">{err}</div>{/if}
    <div class="row">
      <button class="btn-ghost" disabled={busy} onclick={onstay}>Stay</button>
      <button class="btn-danger" disabled={busy} onclick={leave}>{busy ? 'Clearing…' : 'Leave and clear'}</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed; inset: 0; border: none; padding: 0;
    background: rgba(18, 41, 46, 0.28); cursor: default; z-index: 70;
  }
  .layer { position: fixed; inset: 0; display: grid; place-items: center; padding: var(--s-5); z-index: 71; pointer-events: none; }
  .modal {
    width: 100%; max-width: 440px; pointer-events: auto; box-shadow: var(--shadow-pop);
    animation: pop 0.18s ease-out;
  }
  @keyframes pop { from { transform: scale(0.97); opacity: 0; } to { transform: scale(1); opacity: 1; } }
  h2 { font-size: 17px; margin: 0 0 var(--s-3); }
  ul { margin: 0; padding-left: 18px; color: var(--muted); font-size: 13px; display: grid; gap: var(--s-1); }
  .err { margin-top: var(--s-3); }
  .row { display: flex; justify-content: flex-end; gap: var(--s-3); flex-wrap: wrap; margin-top: var(--s-5); }
  @media (prefers-reduced-motion: reduce) { .modal { animation: none; } }
</style>
