<script>
  // A failed action: the one-line `message`, plus an optional `hint` (a longer,
  // operator-facing explanation from the daemon) revealed on demand.
  let { message, hint = null } = $props();
  let open = $state(false);
</script>

{#if message}
  <div class="err">
    <div class="line">
      <span>{message}</span>
      {#if hint}
        <button type="button" class="more" onclick={() => (open = !open)}>
          {open ? 'Hide details' : 'Why?'}
        </button>
      {/if}
    </div>
    {#if hint && open}<p class="hint">{hint}</p>{/if}
  </div>
{/if}

<style>
  .line {
    display: flex;
    align-items: baseline;
    gap: var(--s-3);
  }
  .line span {
    flex: 1 1 auto;
  }
  .more {
    flex: none;
    background: transparent;
    border: 0;
    padding: 0;
    color: inherit;
    font: inherit;
    font-weight: 600;
    text-decoration: underline;
    cursor: pointer;
    white-space: nowrap;
  }
  .hint {
    margin: var(--s-2) 0 0;
    padding-top: var(--s-2);
    border-top: 1px solid color-mix(in srgb, var(--danger) 30%, transparent);
    font-weight: 400;
    line-height: 1.45;
  }
</style>
