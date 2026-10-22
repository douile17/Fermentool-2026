<script>
  import { app } from '../../lib/state.svelte.js';
  import { post } from '../../lib/api.js';
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import QRCode from 'qrcode';
  import Icon from '../../components/Icon.svelte';
  import SettingsCard from './SettingsCard.svelte';

  // `group` (passed to every section) is not shown here: the side menu says it.

  // The people who run experiments, each with their own ntfy topic (phone
  // push) and/or Teams webhook. Applied at once (the notifier reads the live
  // config).
  let loaded = $state(false);
  let people = $state([]);
  let ntfyServer = $state('https://ntfy.sh');
  let err = $state(null);
  let msg = $state(null);
  let testing = $state(null); // index being tested
  let menuFor = $state(null); // index whose "…" menu is open

  $effect(() => {
    loadConfig()
      .then((c) => {
        ntfyServer = c.notify?.ntfy_server || 'https://ntfy.sh';
        people = (c.notify?.people ?? []).map((p) => ({
          name: p.name,
          ntfy_topic: p.ntfy_topic ?? '',
          webhook: p.webhook ?? '',
          showTeams: !!p.webhook,
          saved: { name: p.name, ntfy_topic: p.ntfy_topic ?? '', webhook: p.webhook ?? '' },
        }));
        loaded = true;
      })
      .catch((e) => (err = e.message));
  });

  function addPerson() {
    people.push({ name: '', ntfy_topic: '', webhook: '', showTeams: false });
  }
  // The person a running run's alerts go to: removing them silences that
  // run, so it takes a second click.
  let confirmRemove = $state(null);
  const activeResponsible = $derived(app.status?.active?.responsible?.trim().toLowerCase() ?? null);
  function removePerson(i) {
    const isResponsible = activeResponsible && people[i].name.trim().toLowerCase() === activeResponsible;
    if (isResponsible && confirmRemove !== i) {
      confirmRemove = i;
      return;
    }
    confirmRemove = null;
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
      msg = 'Topic copied.';
    } catch {
      err = 'Copy failed: select the topic and copy it by hand.';
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
  // A row being edited (a topic cleared to paste a new one) keeps its last
  // saved channels meanwhile: dropping it removed the person, and a run they
  // were responsible for stopped alerting them.
  const current = (p) => ({ name: p.name.trim(), ntfy_topic: p.ntfy_topic.trim(), webhook: p.webhook.trim() });
  const snapshot = $derived(
    JSON.stringify({
      people: people.map((p) => (missing(p) ? p.saved : current(p))).filter(Boolean),
      ntfy_server: ntfyServer.trim() || 'https://ntfy.sh',
    }),
  );
  let savedSnapshot = null; // what the daemon has
  let saveState = $state('saved'); // 'saving' | 'saved' | 'error'
  let saveErr = $state(null);
  $effect(() => {
    const snap = snapshot;
    if (!loaded) return;
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
      await patchConfig((c) => (c.notify = JSON.parse(snap)));
      for (const p of people) if (!missing(p)) p.saved = current(p);
      savedSnapshot = snap;
      saveState = 'saved';
    } catch (e) {
      saveErr = e.message;
      saveState = 'error';
    }
  }

  async function testPerson(i, alarm = false) {
    testing = i;
    err = null;
    msg = null;
    const p = people[i];
    try {
      await post('/api/notify/test', {
        name: p.name.trim(),
        ntfy_topic: p.ntfy_topic.trim(),
        webhook: p.webhook.trim(),
        alarm,
      });
      msg = alarm
        ? `Test alarm sent to ${p.name.trim() || 'this person'}: it comes back every minute (5 times at most) until you press Acknowledge on the phone, or here at the top of the page.`
        : `Test sent to ${p.name.trim() || 'this person'}: check the phone${p.webhook.trim() ? ' and Teams' : ''}.`;
    } catch (e) {
      err = e.message;
    }
    testing = null;
  }
</script>

<svelte:window onclick={() => (menuFor = null)} onkeydown={(e) => e.key === 'Escape' && (menuFor = null)} />

<SettingsCard
  title="Notifications"
  {err}
  {msg}
  loading={!loaded}
>
  <details class="howto">
    <summary>Set up alerts on a phone (ntfy, once)</summary>
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

  <!-- One person a row: name, topic, the next step (Generate while there is
       no topic, then QR), Test; the rest behind "…" (UX audit, 2026-10-22:
       five buttons of one weight, Remove as loud as the others). -->
  <div class="people">
    {#each people as p, i}
      <div class="person">
        <div class="row">
          <input type="text" class="name" placeholder="Name" bind:value={p.name} aria-label="Name" />
          <input type="text" class="mono topic" placeholder="ntfy topic" bind:value={p.ntfy_topic} aria-label="ntfy topic" title={p.ntfy_topic} />
          {#if p.ntfy_topic.trim()}
            <button class="btn-ghost" onclick={() => toggleQr(i)} title="Scan with the phone to subscribe">{qrFor === i ? 'Hide QR' : 'QR'}</button>
          {:else}
            <button class="btn-ghost" onclick={() => generateTopic(p)} title="Make a private, unguessable topic">Generate</button>
          {/if}
          <button class="btn-ghost" disabled={testing === i || !(p.ntfy_topic.trim() || p.webhook.trim())} onclick={() => testPerson(i)}>
            {testing === i ? 'Sending…' : 'Test'}
          </button>
          <div class="more">
            <button class="btn-ghost dots" aria-label="More for {p.name || 'this person'}" aria-haspopup="menu"
              aria-expanded={menuFor === i} onclick={(e) => { e.stopPropagation(); menuFor = menuFor === i ? null : i; }}>
              <Icon name="more" size={16} />
            </button>
            {#if menuFor === i}
              <div class="menu" role="menu">
                <button role="menuitem" onclick={() => { generateTopic(p); menuFor = null; }}>New private topic</button>
                <button role="menuitem" disabled={!p.ntfy_topic.trim()} onclick={() => { testPerson(i, true); menuFor = null; }}>Test alarm</button>
                <button role="menuitem" onclick={() => { p.showTeams = true; menuFor = null; }}>{p.webhook ? 'Teams webhook' : 'Add a Teams webhook'}</button>
                <button role="menuitem" class="danger" onclick={() => { menuFor = null; removePerson(i); }}>Remove {p.name.trim() || 'this person'}</button>
              </div>
            {/if}
          </div>
        </div>
        {#if confirmRemove === i}
          <p class="pending">
            {p.name.trim()} is responsible for the run in progress: once removed, its alerts go to nobody.
            <button class="linkish danger" onclick={() => removePerson(i)}>Remove anyway</button>
            <button class="linkish" onclick={() => (confirmRemove = null)}>Keep</button>
          </p>
        {/if}
        {#if qrFor === i}
          <div class="qr">
            <!-- eslint-disable-next-line svelte/no-at-html-tags -- an SVG drawn here by the qrcode library, no outside text in it -->
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
          <input type="text" class="mono teams" placeholder="Teams Workflow webhook: https://…" bind:value={p.webhook} aria-label="Teams webhook" />
        {/if}
        {#if missing(p)}
          <p class="pending">Not saved yet: needs {missing(p)}.{p.saved ? ' Meanwhile the last saved channels stay in use.' : ''}</p>
        {/if}
      </div>
    {:else}
      <p class="muted">Nobody yet: runs need no responsible and send no alerts.</p>
    {/each}
  </div>

  <details class="howto server">
    <summary>ntfy server</summary>
    <label class="field server-field"><span>Server</span>
      <input type="text" class="mono" bind:value={ntfyServer} />
    </label>
    <p class="field-note">
      The public https://ntfy.sh by default. A lab-hosted ntfy keeps the alerts in-house; the
      app then subscribes on that server instead.
    </p>
  </details>

  {#snippet foot()}
    <button class="btn-ghost" onclick={addPerson}>+ Add a person</button>
    <span class="save-state" class:bad={saveState === 'error'} aria-live="polite">
      {#if saveState === 'saving'}Saving…{:else if saveState === 'error'}Not saved: {saveErr}{:else}✓ Saved{/if}
    </span>
  {/snippet}
</SettingsCard>

<style>
  .people { display: flex; flex-direction: column; margin-top: var(--s-4); border: 1px solid var(--line); border-radius: var(--radius-ctl); }
  .person { padding: var(--s-3) var(--s-4); display: flex; flex-direction: column; gap: var(--s-2); }
  .person + .person { border-top: 1px solid var(--line); }
  .row { display: flex; gap: var(--s-2); align-items: center; }
  .row input { width: auto; min-width: 0; }
  .row .name { flex: 0 0 120px; }
  /* a generated topic (~27 characters) fits whole at this size */
  .row .topic { flex: 1 1 auto; font-size: 13px; }
  .more { position: relative; flex: none; align-self: stretch; display: flex; }
  .dots { width: 38px; padding: 0; display: grid; place-items: center; align-self: stretch; }
  .menu {
    position: absolute; right: 0; top: calc(100% + 4px); z-index: 20; min-width: 200px;
    display: flex; flex-direction: column; padding: var(--s-1);
    background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius-ctl); box-shadow: var(--shadow-pop);
  }
  .menu button {
    font: inherit; font-size: 13px; text-align: left; color: var(--ink); background: none; border: none;
    padding: 7px 10px; border-radius: 6px; cursor: pointer;
  }
  .menu button:not(:disabled):hover { background: var(--surface-sunken); }
  .menu button:focus-visible { outline: 2px solid var(--teal-600); outline-offset: -2px; }
  .menu .danger { color: var(--danger-ink); }
  .person .teams { width: 100%; }
  .linkish.danger { color: var(--danger-ink); margin-left: var(--s-2); }
  .qr {
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
  .pending { margin: 0; font-size: 12px; color: var(--danger-ink); }
  .save-state { font-size: 13px; color: var(--green-600); }
  .save-state.bad { color: var(--danger-ink); }
  .linkish { background: none; border: none; padding: 0; font: inherit; font-size: 12px; color: var(--teal-700); cursor: pointer; }
  .server { margin-top: var(--s-4); }
  .server-field { margin-top: var(--s-2); }
  @media (max-width: 720px) {
    .row { flex-wrap: wrap; }
    .row .name, .row .topic { flex: 1 1 100%; }
  }
</style>
