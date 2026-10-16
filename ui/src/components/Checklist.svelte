<script>
  // What still has to be filled before starting, as a row of items that tick
  // and turn green one by one. The start button beside it stays off until
  // every item is green: nothing (the pump direction above all) can be left
  // on a default nobody looked at.
  let { items = [], title = 'Before starting' } = $props();
  const done = $derived(items.filter((i) => i.done).length);
  const all = $derived(items.length > 0 && done === items.length);

  // An item takes you to its field (`target`: an element id, or several for
  // a pair like the two diameters), done or not,
  // after `go()` when the field is on another step, and lights it up so the
  // eye finds it. The input first: a label's "?" hint is a button too.
  function jump(it) {
    if (!it.target) return;
    it.go?.();
    requestAnimationFrame(() => {
      const els = [it.target].flat().map((id) => document.getElementById(id)).filter(Boolean);
      if (!els.length) return;
      els[0].scrollIntoView({ behavior: 'smooth', block: 'center' });
      (els[0].querySelector('input:not(:disabled), select:not(:disabled)') ?? els[0].querySelector('button:not(:disabled)'))
        ?.focus({ preventScroll: true });
      for (const el of els) {
        el.classList.remove('flash-target');
        void el.offsetWidth; // restart the animation on a second click
        el.classList.add('flash-target');
        setTimeout(() => el.classList.remove('flash-target'), 1700);
      }
    });
  }
</script>

<div class="check" class:all role="group" aria-label={title}>
  <div class="check-head">
    <span class="check-title">{title}</span>
    <span class="check-count mono">{done} of {items.length}</span>
  </div>
  <ul>
    {#each items as it (it.label)}
      <li class:done={it.done} title={it.done ? undefined : it.hint}>
        {#if it.target}
          <button type="button" class="jump" onclick={() => jump(it)}>
            <span class="mark" aria-hidden="true">
              {#if it.done}
                <svg viewBox="0 0 16 16"><path d="M3.5 8.5 6.5 11.5 12.5 4.5" /></svg>
              {/if}
            </span>
            <span>{it.label}</span>
            <span class="sr">{it.done ? 'done' : 'to do'}, go to the field</span>
          </button>
        {:else}
          <span class="mark" aria-hidden="true">
            {#if it.done}
              <svg viewBox="0 0 16 16"><path d="M3.5 8.5 6.5 11.5 12.5 4.5" /></svg>
            {/if}
          </span>
          <span>{it.label}</span>
          <span class="sr">{it.done ? 'done' : 'to do'}</span>
        {/if}
      </li>
    {/each}
  </ul>
</div>

<style>
  .check {
    display: flex; flex-direction: column; gap: var(--s-2);
    padding: var(--s-3) var(--s-4); border-radius: var(--radius-ctl);
    background: var(--surface-sunken);
  }
  .check-head { display: flex; justify-content: space-between; align-items: baseline; gap: var(--s-3); }
  .check-title { font-size: 12.5px; font-weight: 600; color: var(--ink); }
  .check-count { font-size: 12px; color: var(--muted); }
  .check.all .check-count { color: var(--green-600); font-weight: 600; }
  ul { list-style: none; margin: 0; padding: 0; display: flex; flex-wrap: wrap; gap: var(--s-2); }
  li {
    display: inline-flex; align-items: center; gap: 6px;
    padding: 4px 10px 4px 6px; border-radius: 999px;
    border: 1px solid var(--line); background: var(--surface);
    font-size: 13px; color: var(--muted);
    transition: background 0.2s ease, border-color 0.2s ease, color 0.2s ease;
  }
  li:has(.jump) { padding: 0; }
  li:not(.done):has(.jump) { border-color: color-mix(in srgb, var(--measured) 55%, var(--surface)); }
  .jump {
    display: inline-flex; align-items: center; gap: 6px;
    padding: 4px 10px 4px 6px; border: none; border-radius: 999px;
    background: transparent; color: var(--ink); font: inherit; font-size: 13px; cursor: pointer;
  }
  .jump:hover { background: color-mix(in srgb, var(--measured) 10%, var(--surface)); }
  li.done .jump:hover { background: color-mix(in srgb, var(--green-500) 14%, transparent); }
  .jump:focus-visible { outline: 2px solid var(--teal-400); outline-offset: 2px; }
  li.done {
    color: var(--ink);
    background: color-mix(in srgb, var(--lime-300) 42%, var(--surface));
    border-color: color-mix(in srgb, var(--green-500) 45%, var(--surface));
  }
  .mark {
    flex: none; width: 16px; height: 16px; border-radius: 50%;
    display: grid; place-items: center;
    border: 1.5px solid color-mix(in srgb, var(--muted) 60%, transparent);
  }
  li.done .mark { border: none; background: var(--green-500); }
  .mark svg { width: 12px; height: 12px; fill: none; stroke: #fff; stroke-width: 2.2; stroke-linecap: round; stroke-linejoin: round; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }
  @media (prefers-reduced-motion: reduce) { li { transition: none; } }
</style>
