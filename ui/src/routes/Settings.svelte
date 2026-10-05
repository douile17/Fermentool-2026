<script>
  import { get, put, post } from '../lib/api.js';
  import QRCode from 'qrcode';

  let cfg = $state(null);
  let err = $state(null);
  let msg = $state(null);
  let saving = $state(false);
  let stopped = $state(false);

  $effect(() => {
    get('/api/config')
      .then((c) => (cfg = c))
      .catch((e) => (err = e.message));
  });

  // Fields this page owns. serial.path and pump.address live in the connection
  // bar and may have changed there since this page loaded, merge our fields
  // onto the current server config instead of round-tripping a stale snapshot.
  async function save() {
    saving = true;
    err = null;
    msg = null;
    try {
      const fresh = await get('/api/config');
      fresh.port = cfg.port;
      fresh.serial.baud = cfg.serial.baud;
      fresh.serial.allow_simulator = cfg.serial.allow_simulator;
      fresh.resume.grace_minutes = cfg.resume.grace_minutes;
      fresh.resume.prompt = cfg.resume.prompt;
      fresh.log.level = cfg.log.level;
      const r = await put('/api/config', fresh);
      cfg = fresh;
      msg = r.note ? `Saved: ${r.note}` : 'Saved.';
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }

  // Balance: its own card and Save, applied live by the daemon (no restart).
  let ports = $state([]);
  let scalePort = $state('');
  let scaleCustom = $state('');
  let scaleBaud = $state(9600);
  let scaleDensity = $state(1.0);
  let scalePosition = $state('feed');
  let trimLimit = $state(25);
  let scaleErr = $state(null);
  let scaleMsg = $state(null);
  let scaleSaving = $state(false);
  let scaleLoaded = false;

  function rescanPorts() {
    get('/api/serial/ports')
      .then((r) => (ports = r.ports ?? []))
      .catch((e) => (scaleErr = e.message));
  }
  $effect(() => rescanPorts());

  // Seed the balance fields once, from the first config load.
  $effect(() => {
    if (!cfg || scaleLoaded) return;
    scaleLoaded = true;
    const p = cfg.scale?.path?.trim() ?? '';
    scalePort = p;
    scaleCustom = p;
    scaleBaud = cfg.scale?.baud ?? 9600;
    scaleDensity = cfg.scale?.density_g_per_ml ?? 1.0;
    scalePosition = cfg.scale?.position ?? 'feed';
    trimLimit = cfg.scale?.trim_limit_pct ?? 25;
  });

  const knownPort = $derived(scalePort === '' || ports.some((p) => p.name === scalePort));
  const pumpPort = $derived(cfg?.serial?.path ?? '');

  async function saveScale() {
    scaleSaving = true;
    scaleErr = null;
    scaleMsg = null;
    try {
      const path = scalePort === '__custom' ? scaleCustom.trim() : scalePort;
      if (path !== '' && path === pumpPort) throw new Error(`${path} is the pump's port, pick the balance's port`);
      const density = Number(scaleDensity);
      if (!(density > 0)) throw new Error('Liquid density must be a positive number (g/mL)');
      const limit = Number(trimLimit);
      if (!(limit >= 5 && limit <= 100)) throw new Error('Correction limit must be between 5 and 100 %');
      const fresh = await get('/api/config');
      fresh.scale = {
        path,
        baud: Number(scaleBaud),
        density_g_per_ml: density,
        position: scalePosition,
        trim_limit_pct: limit,
      };
      const r = await put('/api/config', fresh);
      cfg.scale = fresh.scale;
      if (path === '') scaleMsg = 'Saved: no balance.';
      else if (r.scale_connected === false)
        scaleMsg = `Saved, but the balance does not answer on ${path} yet. Check the port, the cable and the baud; Fermentool keeps retrying.`;
      else scaleMsg = `Saved: balance on ${path}.`;
    } catch (e) {
      scaleErr = e.message;
    }
    scaleSaving = false;
  }

  // Notifications: the people who run experiments, each with their own ntfy
  // topic (phone push) and/or Teams webhook. Own card and Save, applied at
  // once (the notifier reads the live config).
  let people = $state([]);
  let ntfyServer = $state('https://ntfy.sh');
  let notifyLoaded = false;
  let notifyErr = $state(null);
  let notifyMsg = $state(null);
  let testing = $state(null); // index being tested
  $effect(() => {
    if (!cfg || notifyLoaded) return;
    notifyLoaded = true;
    ntfyServer = cfg.notify?.ntfy_server || 'https://ntfy.sh';
    people = (cfg.notify?.people ?? []).map((p) => ({
      name: p.name,
      ntfy_topic: p.ntfy_topic ?? '',
      webhook: p.webhook ?? '',
      showTeams: !!p.webhook,
    }));
  });
  function addPerson() {
    people.push({ name: '', ntfy_topic: '', webhook: '', showTeams: false });
  }
  function removePerson(i) {
    people.splice(i, 1);
  }
  // Subscribing by QR: the ntfy Android app opens `ntfy://<host>/<topic>`
  // links and subscribes on its own (docs.ntfy.sh, "deep linking"); iOS has
  // no such link, so the panel also shows the topic to copy.
  let qrFor = $state(null); // index whose QR is open
  let qrSvg = $state('');
  function ntfyLink(topic) {
    const server = (ntfyServer.trim() || 'https://ntfy.sh').replace(/\/+$/, '');
    const secure = !server.startsWith('http://');
    const host = server.replace(/^https?:\/\//, '');
    return `ntfy://${host}/${encodeURIComponent(topic.trim())}?display=Fermentool${secure ? '' : '&secure=false'}`;
  }
  async function toggleQr(i) {
    if (qrFor === i) {
      qrFor = null;
      return;
    }
    const topic = people[i].ntfy_topic.trim();
    if (!topic) return;
    qrSvg = await QRCode.toString(ntfyLink(topic), { type: 'svg', margin: 1, errorCorrectionLevel: 'M' });
    qrFor = i;
  }
  async function copyTopic(topic) {
    try {
      await navigator.clipboard.writeText(topic);
      notifyMsg = 'Topic copied.';
    } catch {
      notifyErr = 'Copy failed: select the topic and copy it by hand.';
    }
  }

  // A topic is the only lock on a public ntfy server: make it unguessable.
  function generateTopic(p) {
    const slug = p.name.trim().toLowerCase().normalize('NFD').replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'user';
    const bytes = crypto.getRandomValues(new Uint8Array(8));
    const rand = Array.from(bytes, (b) => 'abcdefghijkmnpqrstuvwxyz23456789'[b % 32]).join('');
    p.ntfy_topic = `fermentool-${slug}-${rand}`;
  }
  // Saved by itself, no button: a moment after the last change, every
  // complete person (a name, and a valid topic or webhook) goes to the
  // daemon. An incomplete row waits, flagged, without holding up the others.
  const TOPIC_OK = /^[A-Za-z0-9_-]{12,}$/;
  function missing(p) {
    const topic = p.ntfy_topic.trim();
    const hook = p.webhook.trim();
    if (!p.name.trim()) return 'a name';
    if (!topic && !hook) return 'a topic (click Generate)';
    if (topic && !TOPIC_OK.test(topic)) return 'a valid topic: 12+ letters, digits, - or _ (click Generate)';
    if (hook && !hook.startsWith('https://')) return 'an https:// Teams webhook';
    return null;
  }
  const notifySnapshot = $derived(
    JSON.stringify({
      people: people
        .filter((p) => !missing(p))
        .map((p) => ({ name: p.name.trim(), ntfy_topic: p.ntfy_topic.trim(), webhook: p.webhook.trim() })),
      ntfy_server: ntfyServer.trim() || 'https://ntfy.sh',
    }),
  );
  let savedSnapshot = null; // what the daemon has
  let saveState = $state('saved'); // 'saving' | 'saved' | 'error'
  let saveErr = $state(null);
  $effect(() => {
    const snap = notifySnapshot;
    if (savedSnapshot === null) {
      savedSnapshot = snap; // just loaded from the daemon
      return;
    }
    if (snap === savedSnapshot) return;
    const timer = setTimeout(() => saveNotify(snap), 700);
    return () => clearTimeout(timer);
  });
  async function saveNotify(snap) {
    saveState = 'saving';
    saveErr = null;
    try {
      const fresh = await get('/api/config');
      fresh.notify = JSON.parse(snap);
      await put('/api/config', fresh);
      cfg.notify = fresh.notify;
      savedSnapshot = snap;
      saveState = 'saved';
    } catch (e) {
      saveErr = e.message;
      saveState = 'error';
    }
  }
  async function testPerson(i, alarm = false) {
    testing = i;
    notifyErr = null;
    notifyMsg = null;
    const p = people[i];
    try {
      await post('/api/notify/test', {
        name: p.name.trim(),
        ntfy_topic: p.ntfy_topic.trim(),
        webhook: p.webhook.trim(),
        alarm,
      });
      notifyMsg = alarm
        ? `Test alarm sent to ${p.name.trim() || 'this person'}: it comes back every minute (5 times at most) until you press Acknowledge on the phone, or here at the top of the page.`
        : `Test sent to ${p.name.trim() || 'this person'}: check the phone${p.webhook.trim() ? ' and Teams' : ''}.`;
    } catch (e) {
      notifyErr = e.message;
    }
    testing = null;
  }

  async function shutdown() {
    try {
      await post('/api/shutdown');
      stopped = true;
    } catch (e) {
      err = e.message;
    }
  }
</script>

<section class="card">
  <div class="card-head"><div><div class="eyebrow">Settings</div><h2>Daemon configuration</h2></div></div>

  {#if err}<div class="err" style="margin-bottom:16px">{err}</div>{/if}
  {#if msg}<div class="ok" style="margin-bottom:16px">{msg}</div>{/if}

  {#if !cfg}
    <p class="muted">Loading…</p>
  {:else}
    <div class="grid">
      <label class="field"><span>API port (restart to apply)</span>
        <input type="number" bind:value={cfg.port} />
      </label>

      <p class="field-note">
        The serial port and pump MODBUS address are set in the connection bar at the top of
        the screen. Baud below is applied the next time you click Connect there.
      </p>

      <label class="field"><span>Baud</span>
        <select bind:value={cfg.serial.baud}>
          {#each [1200, 2400, 4800, 9600] as b}<option value={b}>{b}</option>{/each}
        </select>
      </label>

      <label class="field check">
        <input type="checkbox" bind:checked={cfg.serial.allow_simulator} />
        <span>Allow runs on the pump simulator (bench testing, no real pump)</span>
      </label>

      <label class="field"><span>Resume grace (minutes)</span>
        <input type="number" min="0" bind:value={cfg.resume.grace_minutes} />
      </label>

      <label class="field"><span>Log level</span>
        <select bind:value={cfg.log.level}>
          {#each ['error', 'warn', 'info', 'debug', 'trace'] as l}<option value={l}>{l}</option>{/each}
        </select>
      </label>

      <label class="field check">
        <input type="checkbox" bind:checked={cfg.resume.prompt} />
        <span>Ask before resuming an interrupted run</span>
      </label>
    </div>

    <div class="foot">
      <button class="btn-primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
    </div>
  {/if}
</section>

<section class="card">
  <div class="card-head"><div><div class="eyebrow">Settings</div><h2>Balance</h2></div></div>

  {#if scaleErr}<div class="err" style="margin-bottom:16px">{scaleErr}</div>{/if}
  {#if scaleMsg}<div class="ok" style="margin-bottom:16px">{scaleMsg}</div>{/if}

  {#if !cfg}
    <p class="muted">Loading…</p>
  {:else}
    <p class="field-note" style="margin-bottom:16px">
      The balance (Ohaus Ranger, MT-SICS) that the gravimetric trim reads, under the feed bottle
      or under the receiving vessel. Applied as soon as you save, no restart. Its live weight then shows at the bottom of the
      sidebar.
    </p>
    <div class="grid">
      <label class="field"><span>Port</span>
        <select bind:value={scalePort}>
          <option value="">No balance</option>
          {#each ports as p}
            <option value={p.name} disabled={p.name === pumpPort}>
              {p.name}{p.product ? ` · ${p.product}` : ''}{p.name === pumpPort ? ' (pump)' : ''}
            </option>
          {/each}
          {#if !knownPort && scalePort !== '__custom'}
            <option value={scalePort}>{scalePort} (not detected)</option>
          {/if}
          <option value="__custom">Other port…</option>
        </select>
      </label>

      {#if scalePort === '__custom'}
        <label class="field"><span>Port name</span>
          <input type="text" placeholder="COM5" bind:value={scaleCustom} />
        </label>
      {/if}

      <label class="field"><span>Baud</span>
        <select bind:value={scaleBaud}>
          {#each [1200, 2400, 4800, 9600, 19200, 38400] as b}<option value={b}>{b}</option>{/each}
        </select>
      </label>

      <label class="field"><span>Liquid density (g/mL)</span>
        <input type="number" step="0.01" min="0.5" max="2" bind:value={scaleDensity} />
      </label>

      <label class="field"><span>Balance weighs</span>
        <select bind:value={scalePosition}>
          <option value="feed">the feed bottle (weight falls)</option>
          <option value="receiver">the receiving vessel (weight rises)</option>
        </select>
      </label>

      <label class="field"><span>Correction limit (± %)</span>
        <input type="number" step="1" min="5" max="100" bind:value={trimLimit} />
      </label>
      <p class="field-note">
        How far the balance correction may push the pump before it alarms instead
        (±{Number(trimLimit) || 25} %: factor ×{(1 / (1 + (Number(trimLimit) || 25) / 100)).toFixed(2)} to
        ×{(1 + (Number(trimLimit) || 25) / 100).toFixed(2)}). 25 by default. Wider tolerates a tube
        that delivers far from its calibration, but hides a slipping tube, a leak or a bad reading
        for longer. Can be changed during a run (the other balance settings cannot): widening it
        lifts an alarm raised by the old limit.
      </p>

      {#if scalePosition === 'receiver'}
        <p class="field-note">
          Under the receiving vessel, everything else added to it (pH base, antifoam) counts as
          delivered feed, and samples or evaporation count as missing. Under the feed bottle, only
          the pump is measured.
        </p>
      {/if}

      <p class="field-note">
        Baud must match the balance's own Communications menu (9600 by default on the Ranger 7000).
        Density converts the weighed grams to mL: water and dilute feeds ≈ 1.00.
      </p>
    </div>

    <div class="foot">
      <button class="btn-primary" disabled={scaleSaving} onclick={saveScale}>
        {scaleSaving ? 'Connecting…' : 'Save balance'}
      </button>
      <button class="btn-ghost" onclick={rescanPorts}>Rescan ports</button>
    </div>
  {/if}
</section>

<section class="card">
  <div class="card-head"><div><div class="eyebrow">Settings</div><h2>Notifications</h2></div></div>

  {#if notifyErr}<div class="err" style="margin-bottom:16px">{notifyErr}</div>{/if}
  {#if notifyMsg}<div class="ok" style="margin-bottom:16px">{notifyMsg}</div>{/if}

  {#if !cfg}
    <p class="muted">Loading…</p>
  {:else}
    <p class="field-note" style="margin-bottom:16px">
      Everyone who runs experiments, each with their own phone channel. Starting a run then asks
      who it is for, and its alerts (feed stopped, balance or pump lost, run ended…) go to that
      person only. Alarms ring, routine news stays quiet.
    </p>
    <details class="howto">
      <summary>How to get alerts on your phone with ntfy (once, ~3 min)</summary>
      <ol>
        <li>Install the free <b>ntfy</b> app (Google Play, App Store or F-Droid). No account needed.</li>
        <li>Below, <b>Add a person</b>, type your name, click <b>Generate</b> for a private topic.</li>
        <li>Subscribe the phone: <b>QR</b> and scan it (Android), or in the app <b>+</b> and paste the topic (iPhone).</li>
        <li><b>Test</b>: a message should pop up on your phone. Everything here saves by itself.</li>
        <li>Alarms come back every 3 minutes until someone presses <b>Acknowledge</b> on the notification (or on this page), the problem clears, or the run stops. <b>Test alarm</b> shows how it looks.</li>
        <li>Optional, for alarms at night: in the app, allow it to override "Do not disturb" for urgent messages.</li>
      </ol>
      <p class="field-note">
        The topic is the only key: anyone who knows it can read your alerts. Keep it to yourself;
        Generate makes it unguessable.
      </p>
    </details>

    <div class="people">
      {#each people as p, i}
        <div class="person">
          <input type="text" placeholder="Name" bind:value={p.name} aria-label="Name" />
          <div class="topic">
            <input type="text" class="mono" placeholder="ntfy topic" bind:value={p.ntfy_topic} aria-label="ntfy topic" />
            <button class="btn-ghost" onclick={() => generateTopic(p)} title="Make a private, unguessable topic">Generate</button>
            <button class="btn-ghost" disabled={!p.ntfy_topic.trim()} onclick={() => toggleQr(i)}
                    title="Scan with the phone to subscribe">{qrFor === i ? 'Hide QR' : 'QR'}</button>
          </div>
          <button class="btn-ghost" disabled={testing === i || !(p.ntfy_topic.trim() || p.webhook.trim())}
                  onclick={() => testPerson(i)}>
            {testing === i ? 'Sending…' : 'Test'}
          </button>
          <button class="btn-ghost" disabled={testing === i || !p.ntfy_topic.trim()}
                  onclick={() => testPerson(i, true)}
                  title="An alarm that comes back every minute until you press Acknowledge">Test alarm</button>
          <button class="btn-ghost" onclick={() => removePerson(i)} aria-label="Remove {p.name}">Remove</button>
          {#if qrFor === i}
            <div class="qr">
              <div class="qr-img" role="img" aria-label="QR code to subscribe to {p.ntfy_topic}">{@html qrSvg}</div>
              <div class="qr-help">
                <p><b>Android:</b> scan with the phone camera (ntfy installed): the app opens and subscribes by itself.</p>
                <p><b>iPhone:</b> in ntfy, <b>+</b>, then paste this topic:</p>
                <p class="mono topic-line">{p.ntfy_topic.trim()}
                  <button class="btn-ghost" onclick={() => copyTopic(p.ntfy_topic.trim())}>Copy</button></p>
                <p class="field-note">Then <b>Test</b>.</p>
              </div>
            </div>
          {/if}
          {#if p.showTeams}
            <input type="text" class="mono teams" placeholder="Teams Workflow webhook (optional): https://…"
                   bind:value={p.webhook} aria-label="Teams webhook" />
          {:else}
            <button class="linkish teams" onclick={() => (p.showTeams = true)}>+ Teams webhook (optional)</button>
          {/if}
          {#if missing(p)}
            <p class="pending">Not saved yet: needs {missing(p)}.</p>
          {/if}
        </div>
      {:else}
        <p class="muted">Nobody yet: runs need no responsible and send no alerts.</p>
      {/each}
    </div>

    <details class="howto" style="margin-top:16px">
      <summary>ntfy server</summary>
      <label class="field" style="margin-top:8px"><span>Server</span>
        <input type="text" class="mono" bind:value={ntfyServer} />
      </label>
      <p class="field-note">
        The public https://ntfy.sh by default. A lab-hosted ntfy keeps the alerts in-house; the
        app then subscribes on that server instead.
      </p>
    </details>

    <div class="foot">
      <button class="btn-ghost" onclick={addPerson}>Add a person</button>
      <span class="save-state" class:bad={saveState === 'error'} aria-live="polite">
        {#if saveState === 'saving'}Saving…{:else if saveState === 'error'}Not saved: {saveErr}{:else}✓ Saved{/if}
      </span>
    </div>
  {/if}
</section>

<section class="card">
  <div class="card-head"><div><div class="eyebrow">Danger zone</div><h2>Stop the daemon</h2></div></div>
  <p class="muted">
    Stops the control loop and the API. A running pump keeps its last commanded speed;
    restart the daemon to resume the run.
  </p>
  <button class="btn-danger" onclick={shutdown}>Shut down daemon</button>
  {#if stopped}
    <div class="ok" style="margin-top:16px">Daemon stopped. This page is now offline.</div>
  {/if}
</section>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: var(--s-4);
  }
  .field.check { flex-direction: row; align-items: center; gap: var(--s-2); }
  .field.check span { color: var(--ink); font-size: 13px; }
  .field-note {
    grid-column: 1 / -1;
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }
  .foot { margin-top: var(--s-6); display: flex; gap: var(--s-3); flex-wrap: wrap; }
  .muted { color: var(--muted); }
  .people { display: flex; flex-direction: column; gap: var(--s-3); margin-top: var(--s-4); }
  .person { display: grid; grid-template-columns: 160px 1fr auto auto auto; gap: var(--s-2); align-items: center; }
  .person input { min-width: 0; }
  .topic { display: flex; gap: var(--s-2); min-width: 0; }
  .topic input { flex: 1; }
  .person .teams { grid-column: 2 / -1; justify-self: start; }
  .person input.teams { justify-self: stretch; }
  .qr {
    grid-column: 1 / -1;
    display: flex;
    gap: var(--s-4);
    align-items: center;
    flex-wrap: wrap;
    padding: var(--s-3);
    background: var(--surface-sunken);
    border-radius: var(--radius-ctl);
  }
  /* White behind the code whatever the theme: phone cameras need the contrast. */
  .qr-img { width: 200px; height: 200px; background: #fff; padding: 6px; border-radius: 6px; flex: none; }
  .qr-img :global(svg) { width: 100%; height: 100%; display: block; }
  .qr-help { font-size: 13px; flex: 1; min-width: 220px; }
  .qr-help p { margin: 0 0 var(--s-2); }
  .topic-line { display: flex; align-items: center; gap: var(--s-2); word-break: break-all; }
  .pending { grid-column: 1 / -1; margin: 0; font-size: 12px; color: var(--danger); }
  .save-state { align-self: center; font-size: 13px; color: var(--green-600); }
  .save-state.bad { color: var(--danger); }
  .linkish { background: none; border: none; padding: 0; font: inherit; font-size: 12px; color: var(--teal-700); cursor: pointer; }
  .howto { font-size: 13px; }
  .howto summary { cursor: pointer; color: var(--teal-700); }
  .howto ol { margin: var(--s-2) 0; padding-left: var(--s-5); line-height: 1.6; }
  @media (max-width: 720px) {
    .person { grid-template-columns: 1fr auto auto auto; }
    .person input:first-child { grid-column: 1 / -1; }
  }
  .ok {
    color: var(--green-600);
    background: color-mix(in srgb, var(--green-500) 12%, var(--surface));
    border-radius: var(--radius-ctl);
    padding: var(--s-2) var(--s-3);
    font-size: 13px;
  }
</style>
