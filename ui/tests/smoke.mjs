// Smoke test of the UI served by a running daemon, in a real (headless)
// browser: the paths that broke once, walked end to end. It changes nothing:
// no run or burst is started, the calibration draft is only looked at, no
// setting is saved, and the browser profile is a throwaway one.
//
//   npm run smoke                       (daemon on http://127.0.0.1:8730)
//   SMOKE_URL=http://127.0.0.1:8731 npm run smoke
//
// Needs Edge or Chrome. Exits 1 on the first failed check, or on any page
// error (an exception in the page fails the run even if every check passed).
import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const URL = process.env.SMOKE_URL ?? 'http://127.0.0.1:8730';
const BROWSERS = [
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  '/usr/bin/microsoft-edge',
  '/usr/bin/google-chrome',
  '/usr/bin/chromium',
];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let passed = 0;
const pageErrors = [];
function check(what, ok, detail = '') {
  if (!ok) {
    console.error(`FAIL  ${what}${detail ? `: ${detail}` : ''}`);
    finish(1);
  }
  passed++;
  console.log(`ok    ${what}`);
}

// --- the daemon must be up, and idle (New run is not reachable during a run)
let status;
try {
  status = await (await fetch(`${URL}/api/status`)).json();
} catch {
  console.error(`FAIL  no daemon answering at ${URL}`);
  process.exit(1);
}
const idle = !status.active;

// --- a throwaway browser
const exe = BROWSERS.find((p) => existsSync(p));
if (!exe) {
  console.error('FAIL  no Edge or Chrome found');
  process.exit(1);
}
const port = 9300 + Math.floor(Math.random() * 500);
const profile = mkdtempSync(join(tmpdir(), 'ft-smoke-'));
const browser = spawn(exe, ['--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, '--window-size=1400,1000', 'about:blank'], { stdio: 'ignore' });
let ws;
function finish(code) {
  try { ws?.close(); } catch { /* closing anyway */ }
  browser.kill();
  setTimeout(() => {
    try { rmSync(profile, { recursive: true, force: true }); } catch { /* the browser may hold it a moment */ }
    process.exit(code);
  }, 500);
  throw new Error('__finished');
}

let target;
for (let i = 0; i < 60 && !target; i++) {
  try { target = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((t) => t.type === 'page'); } catch { /* not up yet */ }
  if (!target) await sleep(250);
}
ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let id = 0;
const pending = new Map();
ws.addEventListener('message', (m) => {
  const d = JSON.parse(m.data);
  if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); }
  if (d.method === 'Runtime.exceptionThrown') pageErrors.push(d.params.exceptionDetails?.exception?.description ?? d.params.exceptionDetails?.text);
});
const cmd = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const js = async (expr) => (await cmd('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true })).result?.result?.value;
const q = (sel) => JSON.stringify(sel);
/** Click the first element matching `sel` whose text includes `text`. */
const click = (sel, text = '') => js(`(() => { const e = [...document.querySelectorAll(${q(sel)})].find((x) => x.textContent.includes(${q(text)})); if (!e) return false; e.click(); return true; })()`);
const type = (sel, value) => js(`(() => { const i = document.querySelector(${q(sel)}); if (!i) return false; i.value = ${q(value)}; i.dispatchEvent(new Event('input', { bubbles: true })); i.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
const greens = () => js(`[...document.querySelectorAll('.step')].map((b) => b.classList.contains('done') ? 'G' : '-').join('')`);
const stepOn = () => js(`document.querySelector('.step.on .step-l')?.textContent`);

try {
  await cmd('Runtime.enable');
  await cmd('Page.enable');
  await cmd('Page.navigate', { url: URL });
  await sleep(2500);
  check('the app loads', await js(`!!document.querySelector('.tab')`));
  // No "runs dry" time: the balance weighs the bottle and its tubing too,
  // so it came out too late (removed 2026-10-21).
  check('the Overview gives no time the bottle runs dry', await js(`!/Empty around|runs dry/i.test(document.body.innerText)`));
  // Change rate only on a running Constant feed run (2026-10-22); looked
  // at, never clicked.
  const hasChangeRate = () => js(`[...document.querySelectorAll('button')].some((b) => b.textContent.trim() === 'Change rate')`);
  if (idle) {
    check('no Change rate without a run', !(await hasChangeRate()));
  } else if (status.active.kind !== 'calibration') {
    const live = await (await fetch(`${URL}/api/runs/${status.active.run_id}`)).json();
    if (live.curve?.params?.kind !== 'constant') check('no Change rate on a run that is not Constant', !(await hasChangeRate()));
    else console.log(`skip  no Change rate check: run ${status.active.run_id} is Constant`);
  }

  if (!idle) {
    console.log(`skip  New run and History: run ${status.active.run_id} is active`);
  } else {
    // --- New run: steps turn green as their fields are filled, not before
    check('New run opens', await click('button', 'New run'));
    await sleep(1200);
    check('a fresh New run has no green step', (await greens()) === '----', await greens());
    await type('#nr-name input', 'smoke');
    if (await js(`!!document.querySelector('#nr-resp select')`)) {
      await js(`(() => { const s = document.querySelector('#nr-resp select'); const o = [...s.options].find((x) => !x.disabled); s.value = o.value; s.dispatchEvent(new Event('change', { bubbles: true })); return 1; })()`);
    }
    await sleep(200);
    check('Run turns green once named, without leaving it', (await greens())[0] === 'G', await greens());
    await click('.step', 'Pump and tube');
    await sleep(400);
    check('Pump and tube stays grey until a direction is chosen', (await greens())[1] === '-');
    await click('#pump-direction button', 'cw');
    await sleep(200);
    // Bottle refill (balance regulation on the feed bottle): nothing picked
    // for the operator; chosen here, the form only, nothing is sent.
    if (await js(`!!document.querySelector('#nr-refill')`)) {
      check('Bottle refill waits for a choice', (await js(`!!document.querySelector('#nr-refill.need')`)) && (await greens())[1] === '-');
      await click('#nr-refill button', 'By hand');
      await sleep(200);
    }
    const tubeNeeded = await js(`!!document.querySelector('#nr-tube .trigger.need')`);
    if (!tubeNeeded) check('Pump and tube turns green with the direction', (await greens())[1] === 'G', await greens());

    // pump models (2026-10-23): New run offers only the tubes calibrated on the feed pump's model
    const calRows = await (await fetch(`${URL}/api/calibrations`)).json();
    const feedModel = status.pump_model ?? 'labq';
    const otherIds = (Array.isArray(calRows) ? calRows : []).filter((c) => (c.pump_model ?? 'labq') !== feedModel).map((c) => c.id);
    // the tube dropdown: opens on its button, searches, closes on Escape
    if (await js(`!!document.querySelector('#nr-tube .chg')`)) {
      await js(`document.querySelector('#nr-tube .chg').click()`);
      await sleep(400);
      check('the tube list opens on Change', await js(`!!document.querySelector('#nr-tube .pop')`));
      check('the tube search has the focus', (await js(`document.activeElement?.getAttribute('aria-label')`)) === 'Find a tube');
      if (otherIds.length) {
        check('the tube list holds no tube calibrated on another pump model', await js(`${JSON.stringify(otherIds)}.every((i) => !document.querySelector('#tube-opt-' + i))`));
      }
      await js(`document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);
      await sleep(300);
      check('Escape closes the tube list', await js(`!document.querySelector('#nr-tube .pop')`));
    }

    await click('.step', 'Feed profile');
    await sleep(400);
    await type('input[aria-label="Hours"]', '2');
    await sleep(900);
    check('Feed profile turns green with a duration', (await greens())[2] === 'G', await greens());
    check('the curve is drawn beside the profile', await js(`!!document.querySelector('.preview svg')`));
    // a time scale under it, in minutes for this 2 h curve (hours past 2 h)
    check('the curve has a time scale, 0 to 120 min', await js(`(() => { const t = [...document.querySelectorAll('.preview svg text')].map((e) => e.textContent.trim()); return t.includes('0') && t.includes('120 min'); })()`));
    await click('.step', 'Check and start');
    await sleep(600);
    check('Check and start shows the curve and a Start button', await js(`!!document.querySelector('.go .preview svg') && [...document.querySelectorAll('button')].some((b) => b.textContent.includes('Start run'))`));

    // --- History: Run again opens New run on its check (broke once)
    check('History opens', await click('.tab', 'History'));
    check('History lists the runs without an error', await js(`fetch('${URL}/api/runs?limit=1').then((r) => r.ok)`));
    check('the calibrations load without an error', await js(`fetch('${URL}/api/calibrations').then((r) => r.ok)`));
    await sleep(1500);
    if (await js(`!!document.querySelector('button.tr')`)) {
      await js(`document.querySelector('button.tr').click()`);
      await sleep(2500);
      check('a past run opens', await js(`!!document.querySelector('.modal.detail')`));
      // The verdict comes first: "On target" / "Off target", or not weighed.
      check('the run detail says how it went first', await js(`/On target|Off target|Not weighed|wrong side/.test(document.querySelector('.modal.detail .say')?.textContent ?? '')`));
      // the curve said up front, start and end in the header (2026-10-22: hard to find behind Settings)
      check('the run detail says its curve in the header', await js(`(() => { const c = [...document.querySelectorAll('.modal.detail .chips .chip')].map((e) => e.textContent); return c.length >= 3 && c.some((t) => /\\d/.test(t)); })()`));
      check('the run detail shows the feed rate chart', await js(`!!document.querySelector('.modal.detail .block svg')`));
      // its time scale is the experiment's (0, 6 h, 12 h…), never the time of day (17:00)
      // Settings open and first, the bottle table folded under it (2026-10-22)
      check('the run detail opens on Settings, the feed bottle folded below', await js(`(() => { const f = [...document.querySelectorAll('.modal.detail details.fold')]; const n = (d) => d.querySelector('.lbl')?.textContent.trim(); const si = f.findIndex((d) => n(d) === 'Settings'), bi = f.findIndex((d) => n(d) === 'Feed bottle'); return si >= 0 && f[si].open && (bi < 0 || (bi > si && !f[bi].open)); })()`));
      check('the run detail Settings says its feed pump', await js(`/^(LabQ|LabV|PDS II, head \\d)$/.test([...document.querySelectorAll('.modal.detail details.fold dl dt')].find((t) => t.textContent.trim() === 'Feed pump')?.nextElementSibling?.textContent.trim() ?? '')`));
      check('the run detail charts count the experiment time, not the clock', await js(`(() => { const t = [...document.querySelectorAll('.modal.detail svg text')].map((e) => e.textContent.trim()); return t.some((x) => / (h|min)$/.test(x)) && !t.some((x) => /^(\\w{3} )?\\d{2}:\\d{2}$/.test(x)); })()`));
      check('its bottle table and journal count it too, no time of day', await js(`![...document.querySelectorAll('.modal.detail .t, .modal.detail .jr .tm')].some((e) => /\\d{1,2}:\\d{2}/.test(e.textContent))`));
      if (await js(`!!document.querySelector('.modal.detail .jr')`)) {
        check('the journal is in words, not event kinds', await js(`![...document.querySelectorAll('.modal.detail .jr b')].some((b) => /_/.test(b.textContent))`));
      }
      check('the close button closes the run detail', await js(`(() => { document.querySelector('.modal.detail .x')?.click(); return true; })()`) && (await sleep(400), await js(`!document.querySelector('.modal.detail')`)));
      await js(`document.querySelector('button.tr').click()`);
      await sleep(2500);
      const canReplay = await js(`(() => { const b = [...document.querySelectorAll('button')].find((x) => x.textContent.includes('Run again')); return !!b && !b.disabled; })()`);
      // a run whose rate never changed asks nothing on Run again (2026-10-22)
      const openId = await js(`document.querySelector('.modal.detail .rid')?.textContent.replace('#', '')`);
      const opened = await (await fetch(`${URL}/api/runs/${openId}`)).json();
      const changed = (opened.setpoint_changes ?? []).length > 0;
      if (canReplay && !changed) {
        await click('button', 'Run again');
        await sleep(400);
        check('Run again asks no rate when it never changed', await js(`!document.querySelector('#ft-again')`));
        await sleep(1100);
        check('Run again lands on New run, Check and start', (await stepOn()) === 'Check and start', await stepOn());
      } else if (canReplay) {
        console.log(`skip  Run again: run ${openId} changed its rate`);
      } else {
        console.log('skip  Run again: this run\'s curve cannot be rebuilt');
      }
    } else {
      console.log('skip  Run again: no run in History');
    }
  }

  // --- Settings, Display: shows its sample, nothing saved
  check('Settings opens', await click('button', 'Settings'));
  await sleep(800);
  check('the Display section shows a tube size', (await click('button', 'Display')) && (await sleep(800), await js(`!!document.querySelector('.sample .pri')`)));
  // Pumps: the cable and every pump on it (2026-10-21). Looked at only:
  // never saved, reconnected nor tested (a Test runs a pump).
  check('the Pumps section opens', (await click('.side button', 'Pumps')) && (await sleep(1200), await js(`!!document.querySelector('.pumps')`)));
  check('it says the cable state first, in a word, above the port', await js(`(() => { const h = document.querySelector('.settings-pane .head .word'); const p = document.querySelector('.conn select.port'); return !!h && !!p && /^(Connected|Not connected|Not answering|Simulator)$/.test(h.textContent.trim()) && (h.compareDocumentPosition(p) & Node.DOCUMENT_POSITION_FOLLOWING) > 0; })()`));
  // Disconnect while the cable is up, Connect while it is down (never clicked here)
  // the state is a tinted band; Disconnect on it has no white box of its own (it sat on the band like a sticker)
  check('the state is a tinted band, Disconnect without a box on it', await js(`(() => { const h = document.querySelector('.settings-pane .head'); const d = h && [...h.querySelectorAll('button')].find((b) => b.textContent.trim() === 'Disconnect'); return !!h && getComputedStyle(h).backgroundColor !== 'rgba(0, 0, 0, 0)' && (!d || getComputedStyle(d).backgroundColor === 'rgba(0, 0, 0, 0)'); })()`));
  check('the cable button says Connect or Disconnect', await js(`(() => { const b = [...document.querySelectorAll('.head button')].map((x) => x.textContent.trim()); return b.includes('Connect') !== b.includes('Disconnect'); })()`));
  check('it lists the feed and the refill pump, each with an address', await js(`document.querySelectorAll('.pumps .item').length === 2 && document.querySelectorAll('.pumps .addr input').length === 2`));
  check(
    'Change opens the refill pump settings and its Test button',
    (await click('.pumps .change', '')) &&
      (await sleep(300), await js(`!!document.querySelector('.pumps .more input') && [...document.querySelectorAll('.pumps .more button')].some((b) => b.textContent.includes('Test 3 s'))`)),
  );
  // Pump models (2026-10-23, mockup v3): each row a photo and a LabQ / LabV /
  // PDS II choice; the cable is fixed at 9600 baud 8E1, no baud to pick.
  check('each pump has its photo and a LabQ, LabV, PDS II choice', await js(`(() => { const it = [...document.querySelectorAll('.pumps .item')]; return it.length === 2 && it.every((i) => { const g = i.querySelector('[role=radiogroup][aria-label$="pump model"]'); const r = g ? [...g.querySelectorAll('[role=radio]')].map((b) => b.textContent.trim()) : []; return !!i.querySelector('.pic img') && r.join(',') === 'LabQ,LabV,PDS II'; }); })()`));
  check('the cable says 9600 baud, no baud to pick', await js(`/9600 baud/.test(document.querySelector('.conn .bus')?.textContent ?? '') && ![...document.querySelectorAll('.settings-pane select')].some((s) => /baud/i.test(s.getAttribute('aria-label') ?? '') || [...s.options].some((o) => /baud/i.test(o.textContent)))`));
  // Max rpm and steps are the model's (2026-10-26): no field to set them; the
  // refill speed steps by its model's rpm step (0.1 on a LabQ, 0.01 else)
  check('no max rpm field: the top speed is the model one', await js(`![...document.querySelectorAll('.pumps label')].some((l) => /max rpm/i.test(l.textContent))`));
  check('the refill speed steps by its pump model', await js(`(() => { const m = [...document.querySelectorAll('.pumps [aria-label="Refill pump model"] [role=radio]')].find((b) => b.getAttribute('aria-checked') === 'true')?.textContent.trim(); const i = document.querySelector('.pumps .more input'); return !!m && !!i && Number(i.step) === (m === 'LabQ' ? 0.1 : 0.01); })()`));
  // PDS II on the refill row shows its Head; then left without Save: the saved model comes back
  const refillModel = `[...document.querySelectorAll('.pumps [aria-label="Refill pump model"] [role=radio]')]`;
  const savedRefill = await js(`${refillModel}.find((b) => b.getAttribute('aria-checked') === 'true')?.textContent.trim()`);
  if (await js(`${refillModel}.find((b) => b.textContent.trim() === 'PDS II')?.disabled ?? true`)) {
    console.log('skip  the refill pump model: locked while a run refills');
  } else {
    await js(`${refillModel}.find((b) => b.textContent.trim() === 'PDS II').click()`);
    await sleep(300);
    check('PDS II on the refill pump shows a Head field and a Head badge', await js(`(() => { const i = document.querySelectorAll('.pumps .item')[1]; return !!i && !!i.querySelector('label.head select') && /^Head \\d$/.test(i.querySelector('.pic .hd')?.textContent.trim() ?? ''); })()`));
    await click('.side button', 'Display');
    await sleep(600);
    await click('.side button', 'Pumps');
    await sleep(1200);
    check('left without Save, the refill pump keeps its saved model', (await js(`${refillModel}.find((b) => b.getAttribute('aria-checked') === 'true')?.textContent.trim()`)) === savedRefill, savedRefill);
  }
  check('no Refill pump entry left in the Settings menu', await js(`![...document.querySelectorAll('.side button')].some((b) => b.textContent.trim() === 'Refill pump')`));

  // --- the other sections, looked at only (UX audit, 2026-10-22)
  check('Display applies on the click, no Save', (await click('.side button', 'Display')) && (await sleep(800), await js(`![...document.querySelectorAll('.settings-pane button')].some((b) => b.textContent.trim() === 'Save')`)));
  check('Balance keeps Save off until a change', (await click('.side button', 'Balance')) && (await sleep(1200), await js(`[...document.querySelectorAll('.settings-pane button.save')].every((b) => b.disabled)`)));
  check('Balance says its state in a word, with its Connect or Disconnect', await js(`/^(Connected|Disconnected|Not answering|No balance)$/.test(document.querySelector('.settings-pane .head .word')?.textContent.trim() ?? '') && document.querySelectorAll('.settings-pane .head button').length === 1`));
  check('Crash resume is one switch, no Save, no grace', (await click('.side button', 'Crash resume')) && (await sleep(1200), await js(`document.querySelectorAll('.settings-pane input[type=checkbox]').length === 1 && !document.querySelector('.settings-pane input[type=number]') && ![...document.querySelectorAll('.settings-pane button')].some((b) => b.textContent.trim() === 'Save')`)));
  // the desktop window and installer are built for port 8730: no API port to change here (2026-10-22)
  check('Daemon has the log level and no API port', (await click('.side button', 'Daemon')) && (await sleep(1200), await js(`!!document.querySelector('.settings-pane select') && !document.querySelector('.settings-pane input[type=number]') && !/API port/.test(document.querySelector('.settings-pane').textContent)`)));
  await click('.side button', 'Notifications');
  await sleep(1200);
  if (await js(`!!document.querySelector('.person .dots')`)) {
    check(
      'a person\'s "…" menu holds Remove, out of the row',
      (await click('.person .dots', '')) &&
        (await sleep(200), await js(`!!document.querySelector('.person .menu .danger') && ![...document.querySelectorAll('.person .row > button')].some((b) => b.textContent.includes('Remove'))`)),
    );
    await js(`window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))`);
    await sleep(200);
    check('Escape closes the menu', await js(`!document.querySelector('.person .menu')`));
  }

  // --- Tubing calibration last: it is only looked at, its draft untouched
  await click('button', 'Feeding profile');
  await sleep(500);
  check('Tubing calibration opens', await click('.tab', 'Tubing calibration'));
  await sleep(1800);
  check('the calibration shows its four steps', (await js(`document.querySelectorAll('.steps .step').length`)) === 4);
  // "burst" is the code's word; the page says measurement (2026-10-20)
  check('the calibration says measurement, never burst', await js(`!/burst/i.test(document.querySelector('main')?.innerText ?? document.body.innerText)`));
  // each calibration of the history says the pump model it was measured on
  if (await js(`(() => { const b = document.querySelector('.hist-toggle'); if (b && b.getAttribute('aria-expanded') !== 'true') b.click(); return true; })()`) && (await sleep(400), await js(`!!document.querySelector('table.hist tbody tr')`))) {
    check('a calibration row shows its pump model', await js(`[...document.querySelectorAll('table.hist tbody tr')].every((r) => /^(LabQ|LabV|PDS II)$/.test(r.querySelector('td.pm')?.textContent.trim() ?? ''))`));
    // the "pump" header sits over the pump cells, every column in step
    check('the pump header is over the pump column', await js(`(() => { const hs = [...document.querySelectorAll('table.hist thead th')].map((t) => t.textContent.trim()); const row = document.querySelector('table.hist tbody tr'); const pm = row ? [...row.children].indexOf(row.querySelector('td.pm')) : -1; return pm >= 0 && hs.indexOf('pump') === pm && hs.length === row.children.length; })()`));
  }
  if (await js(`!!document.querySelector('.panel h3')`)) {
    // leaving with a calibration under way asks first; Stay keeps everything
    await click('.tab', 'Overview');
    await sleep(500);
    if (await js(`!!document.querySelector('[role=alertdialog]')`)) {
      check('leaving the calibration asks first', true);
      await click('[role=alertdialog] button', 'Stay');
      await sleep(400);
      check('Stay keeps the calibration on screen', await js(`!!document.querySelector('.steps')`));
    }
  }

  check('no error in the page', pageErrors.length === 0, pageErrors.join(' | '));
  console.log(`\n${passed} checks passed`);
  finish(0);
} catch (e) {
  if (e.message !== '__finished') {
    console.error(`FAIL  ${e.message}`);
    finish(1);
  }
}
