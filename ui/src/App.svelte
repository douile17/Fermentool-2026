<script>
  // Scaffold shell (milestone 1). Milestone 8 replaces this with the real
  // sections: New run · Active run · History · Resume modal · Settings, plus
  // disabled Sensors / Camera / Data-export tabs.
  // See docs/IMPLEMENTATION_PLAN.md §5.

  let status = $state({ state: 'connecting…' });
  let reachable = $state(false);

  async function poll() {
    try {
      const r = await fetch('/api/status');
      if (!r.ok) throw new Error(String(r.status));
      status = await r.json();
      reachable = true;
    } catch {
      reachable = false;
      status = { state: 'daemon API not implemented yet (scaffold)' };
    }
  }

  $effect(() => {
    poll();
    const id = setInterval(poll, 5000);
    return () => clearInterval(id);
  });
</script>

<main>
  <header>
    <h1>Fermentool</h1>
    <span class="tag">pump time-profile controller</span>
    <span class="dot" class:ok={reachable} class:err={!reachable}></span>
  </header>

  <section>
    <h2>Daemon status</h2>
    <pre>{JSON.stringify(status, null, 2)}</pre>
  </section>

  <section class="soon">
    <h2>Coming next</h2>
    <ul>
      <li>New run — curve builder + live preview chart</li>
      <li>Active run — planned vs. actual, event log, stop</li>
      <li>History — past runs, CSV export</li>
      <li>Resume modal — after an interrupted run</li>
    </ul>
  </section>
</main>

<style>
  main {
    max-width: 760px;
    margin: 0 auto;
    padding: 2rem 1.25rem;
  }
  header {
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
  }
  h1 {
    margin: 0;
  }
  .tag {
    color: var(--muted);
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    align-self: center;
    background: var(--muted);
  }
  .dot.ok {
    background: var(--ok);
  }
  .dot.err {
    background: var(--err);
  }
  section {
    margin-top: 2rem;
  }
  .soon {
    color: var(--muted);
  }
</style>
