<script>
  // The frame every Settings section uses: same head, same message lines,
  // same loading state, so a new section only writes its fields.
  let { group, title, intro = null, err = null, msg = null, loading = false, children, foot = null } = $props();
</script>

<section class="card">
  <div class="card-head"><div><div class="eyebrow">{group}</div><h2>{title}</h2></div></div>

  {#if err}<div class="err msg">{err}</div>{/if}
  {#if msg}<div class="ok msg">{msg}</div>{/if}

  {#if loading}
    <p class="muted">Loading…</p>
  {:else}
    {#if intro}<p class="field-note intro">{intro}</p>{/if}
    {@render children()}
    {#if foot}<div class="foot">{@render foot()}</div>{/if}
  {/if}
</section>

<style>
  .msg { margin-bottom: var(--s-4); }
  /* outranks the shared `.settings-pane .field-note { margin: 0 }` */
  .card .intro { margin-bottom: var(--s-4); }
  .foot { margin-top: var(--s-6); display: flex; gap: var(--s-3); flex-wrap: wrap; align-items: center; }
</style>
