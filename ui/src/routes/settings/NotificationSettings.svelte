<script>
  import { post } from '../../lib/api.js';
  import { loadConfig, patchConfig } from '../../lib/config.js';
  import QRCode from 'qrcode';
  import SettingsCard from './SettingsCard.svelte';

  let { group } = $props();

  // The people who run experiments, each with their own ntfy topic (phone
  // push) and/or Teams webhook. Applied at once (the notifier reads the live
  // config).
  let loaded = $state(false);
  let people = $state([]);
  let ntfyServer = $state('https://ntfy.sh');
  let err = $state(null);
  let msg = $state(null);
  let testing = $state(null); // index being tested

  $effect(() => {
    loadConfig()
      .then((c) => {
        ntfyServer = c.notify?.ntfy_server || 'https://ntfy.sh';
        people = (c.notify?.people ?? []).map((p) => ({
          name: p.name,
          ntfy_topic: p.ntfy_topic ?? '',
          webhook: p.webhook ?? '',
          showTeams: !!p.webhook,
        }));
        loaded = true;
      })
      .catch((e) => (err = e.message));
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
  const snapshot = $derived(
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

<SettingsCard
  {group}
  title="Notifications"
  intro="Everyone who runs experiments, each with their own phone channel. Starting a run then asks who it is for, and its alerts (feed stopped, balance or pump lost, run ended…) go to that person only. Alarms ring, routine news stays quiet."
  {err}
  {msg}
  loading={!loaded}
>
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
    <button class="btn-ghost" onclick={addPerson}>Add a person</button>
    <span class="save-state" class:bad={saveState === 'error'} aria-live="polite">
      {#if saveState === 'saving'}Saving…{:else if saveState === 'error'}Not saved: {saveErr}{:else}✓ Saved{/if}
    </span>
  {/snippet}
</SettingsCard>

<style>
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
  .save-state { font-size: 13px; color: var(--green-600); }
  .save-state.bad { color: var(--danger); }
  .linkish { background: none; border: none; padding: 0; font: inherit; font-size: 12px; color: var(--teal-700); cursor: pointer; }
  .server { margin-top: var(--s-4); }
  .server-field { margin-top: var(--s-2); }
  @media (max-width: 720px) {
    .person { grid-template-columns: 1fr auto auto auto; }
    .person input:first-child { grid-column: 1 / -1; }
  }
</style>
