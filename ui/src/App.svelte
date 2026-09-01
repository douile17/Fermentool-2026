<script>
  // Scaffold shell (milestone 3). The layout language — left rail, rounded card
  // grid, hero area chart, progress ring — is the target; milestone 8 fills the
  // cards with live data and adds New run / Active run / History / Settings.
  // Light theme, palette per docs/DESIGN.md.

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

  const nav = [
    ['Overview', '▦', true],
    ['New run', '＋', false],
    ['Active run', '◉', false],
    ['History', '≣', false],
    ['Sensors', '⌁', false],
    ['Settings', '⚙', false],
  ];

  const roadmap = [
    ['Curve engine', 'linear · exponential · sigmoid · step · custom', true],
    ['MODBUS + pump', 'frames, Pump/SimPump, serial port picker', true],
    ['Journal + resume', 'SQLite tick log, time-correct restart', false],
    ['Live UI', 'new-run builder, planned vs. actual chart', false],
  ];
</script>

<div class="app">
  <aside class="rail">
    <div class="logo">
      <span class="mark" aria-hidden="true"></span>
      <span class="word">Fermentool</span>
    </div>

    <nav>
      {#each nav as [label, icon, active]}
        <button class="nav-item" class:active disabled={!active}>
          <span class="ic" aria-hidden="true">{icon}</span>
          {label}
        </button>
      {/each}
    </nav>

    <div class="rail-foot">
      <span class="pill" class:on={reachable}>
        <span class="dot"></span>
        {reachable ? 'daemon online' : 'daemon offline'}
      </span>
    </div>
  </aside>

  <div class="main">
    <header class="topbar">
      <h1>Overview</h1>
      <input class="search" type="search" placeholder="Search — coming soon" disabled />
      <span class="ver">scaffold · m3</span>
    </header>

    <div class="grid">
      <section class="card hero">
        <div class="card-head">
          <h2>Planned profile</h2>
          <span class="chip">no run</span>
        </div>
        <div class="reading"><strong>—</strong> rpm</div>
        <svg class="spark" viewBox="0 0 600 160" preserveAspectRatio="none" aria-hidden="true">
          <defs>
            <linearGradient id="fill" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stop-color="var(--lime-300)" stop-opacity="0.55" />
              <stop offset="100%" stop-color="var(--lime-300)" stop-opacity="0" />
            </linearGradient>
          </defs>
          <path
            d="M0,150 C120,150 150,120 220,96 C300,68 340,40 420,30 C500,22 560,16 600,14 L600,160 L0,160 Z"
            fill="url(#fill)"
          />
          <path
            d="M0,150 C120,150 150,120 220,96 C300,68 340,40 420,30 C500,22 560,16 600,14"
            fill="none"
            stroke="var(--teal-700)"
            stroke-width="2.5"
          />
        </svg>
      </section>

      <section class="card">
        <div class="card-head"><h2>Run clock</h2></div>
        <div class="ring" style="--pct: 0">
          <span>idle</span>
        </div>
      </section>

      <section class="card">
        <div class="card-head"><h2>Build roadmap</h2></div>
        <ul class="roadmap">
          {#each roadmap as [name, detail, done]}
            <li class:done>
              <span class="tick" aria-hidden="true">{done ? '✓' : ''}</span>
              <span><strong>{name}</strong> — {detail}</span>
            </li>
          {/each}
        </ul>
      </section>

      <section class="card">
        <div class="card-head"><h2>Daemon status</h2></div>
        <pre>{JSON.stringify(status, null, 2)}</pre>
      </section>
    </div>
  </div>
</div>

<style>
  .app {
    display: grid;
    grid-template-columns: 208px 1fr;
    min-height: 100vh;
  }

  /* Left rail */
  .rail {
    display: flex;
    flex-direction: column;
    gap: var(--s-5);
    padding: var(--s-5) var(--s-3);
    background: var(--surface);
    border-right: 1px solid var(--line);
  }
  .logo {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    padding: 0 var(--s-2);
  }
  .mark {
    width: 28px;
    height: 28px;
    border-radius: 8px;
    background: linear-gradient(135deg, var(--teal-700), var(--green-500));
  }
  .word {
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    padding: var(--s-2) var(--s-3);
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--muted);
    text-align: left;
    border: none;
  }
  .nav-item .ic {
    width: 18px;
    text-align: center;
    opacity: 0.9;
  }
  .nav-item:not(:disabled):hover {
    background: var(--surface-sunken);
    color: var(--ink);
  }
  .nav-item.active {
    background: color-mix(in srgb, var(--teal-700) 12%, var(--surface));
    color: var(--teal-700);
    font-weight: 600;
  }
  .nav-item:disabled {
    cursor: default;
    opacity: 0.55;
  }
  .rail-foot {
    margin-top: auto;
    padding: 0 var(--s-2);
  }

  .pill {
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
    padding: var(--s-1) var(--s-3);
    border: 1px solid var(--line);
    border-radius: 999px;
    color: var(--muted);
    font-size: 0.78rem;
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

  /* Main column */
  .main {
    padding: var(--s-5) var(--s-6) var(--s-7);
  }
  .topbar {
    display: flex;
    align-items: center;
    gap: var(--s-4);
    margin-bottom: var(--s-5);
  }
  .topbar h1 {
    margin-right: var(--s-2);
  }
  .search {
    flex: 1;
    max-width: 420px;
    padding: var(--s-2) var(--s-3);
    border: 1px solid var(--line);
    border-radius: 999px;
    background: var(--surface);
    color: var(--muted);
  }
  .ver {
    margin-left: auto;
    color: var(--muted);
    font-size: 0.78rem;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: var(--s-4);
  }
  .card-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--s-3);
  }
  .hero {
    grid-column: 1 / -1;
  }
  .chip,
  .reading strong {
    font-variant-numeric: tabular-nums;
  }
  .chip {
    font-size: 0.72rem;
    color: var(--muted);
    background: var(--surface-sunken);
    padding: 2px var(--s-2);
    border-radius: 999px;
  }
  .reading {
    font-size: 1.6rem;
    color: var(--muted);
  }
  .reading strong {
    color: var(--ink);
    font-size: 2.4rem;
    font-weight: 650;
  }
  .spark {
    width: 100%;
    height: 150px;
    margin-top: var(--s-3);
    display: block;
  }

  .ring {
    --size: 132px;
    width: var(--size);
    height: var(--size);
    margin: var(--s-2) auto 0;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: var(--muted);
    font-size: 0.85rem;
    background:
      radial-gradient(closest-side, var(--surface) 78%, transparent 79% 100%),
      conic-gradient(
        var(--green-500) calc(var(--pct) * 1%),
        var(--surface-sunken) 0
      );
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

  @media (max-width: 720px) {
    .app {
      grid-template-columns: 1fr;
    }
    .rail {
      flex-direction: row;
      align-items: center;
      border-right: none;
      border-bottom: 1px solid var(--line);
      overflow-x: auto;
    }
    .rail-foot {
      margin-top: 0;
    }
    .grid {
      grid-template-columns: 1fr;
    }
  }
</style>
