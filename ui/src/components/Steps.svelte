<script>
  // The steps of a setup (tubing calibration, new run) as arrow tabs: each
  // points at the next, so the order reads by itself. A step shows its number,
  // a tick once complete, and a one-line summary of what was chosen there.
  /** @type {{ steps: { n:number, label:string, sub?:string, done?:boolean, disabled?:boolean }[], current:number, onpick:(n:number) => void, label?:string }} */
  let { steps, current, onpick, label = 'Steps' } = $props();
</script>

<nav class="steps" aria-label={label}>
  {#each steps as s (s.n)}
    <button
      class="step"
      class:on={current === s.n}
      class:done={s.done}
      disabled={s.disabled}
      aria-current={current === s.n ? 'step' : undefined}
      onclick={() => onpick(s.n)}
    >
      <span class="step-n">{s.done ? '✓' : s.n}</span>
      <span class="step-t">
        <span class="step-l">{s.label}</span>
        {#if s.sub}<span class="step-s">{s.sub}</span>{/if}
      </span>
    </button>
  {/each}
</nav>

<style>
  /* On a white band like the cards: the light green of a done step and the
     grey of a coming one both stand out there, not on the page's tint. */
  .steps {
    display: flex; gap: 4px; padding: 6px;
    background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius);
  }
  .step {
    --tip: 16px;
    position: relative; flex: 1 1 0;
    display: flex; align-items: center; gap: var(--s-3); min-width: 0; text-align: left;
    padding: var(--s-3) calc(var(--tip) + var(--s-2)) var(--s-3) calc(var(--tip) + var(--s-3));
    margin-left: calc(-1 * var(--tip) + 4px);
    border: none; border-radius: 0;
    background: var(--surface-sunken); color: var(--ink);
    clip-path: polygon(0 0, calc(100% - var(--tip)) 0, 100% 50%, calc(100% - var(--tip)) 100%, 0 100%, var(--tip) 50%);
    transition: background 0.2s ease;
  }
  /* the first one starts flat, rounded on the left */
  .step:first-child {
    margin-left: 0; padding-left: var(--s-4);
    border-radius: var(--radius-ctl) 0 0 var(--radius-ctl);
    clip-path: polygon(0 0, calc(100% - var(--tip)) 0, 100% 50%, calc(100% - var(--tip)) 100%, 0 100%);
  }
  /* the last one ends flat, rounded on the right: the first one mirrored */
  .step:last-child {
    padding-right: var(--s-4);
    border-radius: 0 var(--radius-ctl) var(--radius-ctl) 0;
    clip-path: polygon(0 0, 100% 0, 100% 100%, 0 100%, var(--tip) 50%);
  }
  .step:last-child.on::after { right: var(--s-3); }
  .step:not(:disabled):hover { filter: brightness(0.97); }
  .step:disabled { opacity: 0.55; }
  .step.on { background: color-mix(in srgb, var(--teal-700) 14%, var(--surface)); }
  /* complete: the light green of the rest of the app */
  .step.done { background: color-mix(in srgb, var(--lime-300) 55%, var(--surface)); }
  /* the step on screen: a teal line under it, inside the arrow */
  .step.on::after {
    content: ''; position: absolute; left: calc(var(--tip) + 4px); right: calc(var(--tip) + 4px); bottom: 0;
    height: 3px; border-radius: 3px 3px 0 0; background: var(--teal-700);
  }
  .step:first-child.on::after { left: var(--s-3); }
  .step:focus-visible { outline: none; filter: brightness(0.94); }
  .step-n {
    flex: none; width: 26px; height: 26px; border-radius: 50%; display: grid; place-items: center;
    font-size: 12.5px; font-weight: 600; background: var(--surface); color: var(--muted);
  }
  .step.on .step-n { background: var(--teal-700); color: #fff; }
  .step.done .step-n, .step.on.done .step-n { background: var(--green-500); color: #fff; }
  .step-t { display: flex; flex-direction: column; min-width: 0; }
  .step-l { font-weight: 600; }
  .step-s { font-size: 12px; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  @media (max-width: 760px) {
    .step-s { display: none; }
  }
  @media (prefers-reduced-motion: reduce) { .step { transition: none; } }
</style>
