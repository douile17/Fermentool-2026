<script>
  // Scaffold shell (milestone 1). Milestone 8 replaces the body with the real
  // sections: New run · Active run · History · Resume modal · Settings, plus
  // disabled Sensors / Camera / Data-export tabs. Design: docs/DESIGN.md.

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

  const roadmap = [
    ['Curve engine', 'linear · exponential fed-batch · sigmoid · step · custom', true],
    ['MODBUS link', 'CRC + framing verified against the LabQ vendor frames', true],
    ['New run', 'curve builder with a live preview chart', false],
    ['Active run', 'planned vs. actual, event log, stop', false],
    ['History', 'past runs, CSV export', false],
    ['Resume', 'time-correct restart after an interruption', false],
  ];
</script>

<div class="shell">
  <header>
    <div class="brand">
      <span class="mark" aria-hidden="true"></span>
      <div>
        <h1>Fermentool</h1>
        <p class="sub">pump time-profile controller</p>
      </div>
    </div>
    <span class="pill" class:on={reachable}>
      <span class="dot"></span>
      {reachable ? 'daemon online' : 'daemon offline'}
    </span>
  </header>

  <main>
    <section class="card">
      <h2>Daemon status</h2>
      <pre>{JSON.stringify(status, null, 2)}</pre>
    </section>

    <section class="card">
      <h2>Build roadmap</h2>
      <ul class="roadmap">
        {#each roadmap as [name, detail, done]}
          <li class:done>
            <span class="tick" aria-hidden="true">{done ? '✓' : ''}</span>
            <span><strong>{name}</strong> — {detail}</span>
          </li>
        {/each}
      </ul>
    </section>
  </main>
</div>

<style>
  .shell {
    max-width: 820px;
    margin: 0 auto;
    padding: var(--s-6) var(--s-5) var(--s-7);
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--s-4);
    margin-bottom: var(--s-6);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: var(--s-3);
  }

  .mark {
    width: 34px;
    height: 34px;
    border-radius: 10px;
    background: linear-gradient(135deg, var(--teal-700), var(--green-500));
    box-shadow: var(--shadow-sm);
  }

  .sub {
    margin: 2px 0 0;
    color: var(--muted);
    font-size: 0.85rem;
  }

  .pill {
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
    padding: var(--s-1) var(--s-3);
    border: 1px solid var(--line);
    border-radius: 999px;
    color: var(--muted);
    font-size: 0.8rem;
    white-space: nowrap;
  }
  .pill .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--muted);
  }
  .pill.on {
    color: var(--green-600);
    border-color: color-mix(in srgb, var(--green-500) 45%, var(--line));
  }
  .pill.on .dot {
    background: var(--green-500);
  }

  main {
    display: grid;
    gap: var(--s-4);
  }

  h2 {
    margin-bottom: var(--s-3);
  }

  .roadmap {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--s-2);
  }
  .roadmap li {
    display: flex;
    align-items: baseline;
    gap: var(--s-3);
    color: var(--muted);
  }
  .roadmap li.done {
    color: var(--ink);
  }
  .tick {
    flex: none;
    width: 18px;
    height: 18px;
    border-radius: 5px;
    display: grid;
    place-items: center;
    font-size: 0.7rem;
    color: var(--white);
    background: var(--surface-sunken);
  }
  .roadmap li.done .tick {
    background: var(--green-500);
  }
</style>
