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

    // the tube dropdown: opens on its button, searches, closes on Escape
    if (await js(`!!document.querySelector('#nr-tube .chg')`)) {
      await js(`document.querySelector('#nr-tube .chg').click()`);
      await sleep(400);
      check('the tube list opens on Change', await js(`!!document.querySelector('#nr-tube .pop')`));
      check('the tube search has the focus', (await js(`document.activeElement?.getAttribute('aria-label')`)) === 'Find a tube');
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
    await click('.step', 'Check and start');
    await sleep(600);
    check('Check and start shows the curve and a Start button', await js(`!!document.querySelector('.go .preview svg') && [...document.querySelectorAll('button')].some((b) => b.textContent.includes('Start run'))`));

    // --- History: Run again opens New run on its check (broke once)
    check('History opens', await click('.tab', 'History'));
    await sleep(1500);
    if (await js(`!!document.querySelector('button.tr')`)) {
      await js(`document.querySelector('button.tr').click()`);
      await sleep(2500);
      check('a past run opens', await js(`!!document.querySelector('.modal.detail')`));
      // The verdict comes first: "On target" / "Off target", or not weighed.
      check('the run detail says how it went first', await js(`/On target|Off target|Not weighed|wrong side/.test(document.querySelector('.modal.detail .say')?.textContent ?? '')`));
      check('the run detail shows the feed rate chart', await js(`!!document.querySelector('.modal.detail .block svg')`));
      if (await js(`!!document.querySelector('.modal.detail .jr')`)) {
        check('the journal is in words, not event kinds', await js(`![...document.querySelectorAll('.modal.detail .jr b')].some((b) => /_/.test(b.textContent))`));
      }
      check('the close button closes the run detail', await js(`(() => { document.querySelector('.modal.detail .x')?.click(); return true; })()`) && (await sleep(400), await js(`!document.querySelector('.modal.detail')`)));
      await js(`document.querySelector('button.tr').click()`);
      await sleep(2500);
      const canReplay = await js(`(() => { const b = [...document.querySelectorAll('button')].find((x) => x.textContent.includes('Run again')); return !!b && !b.disabled; })()`);
      if (canReplay) {
        await click('button', 'Run again');
        await sleep(1500);
        check('Run again lands on New run, Check and start', (await stepOn()) === 'Check and start', await stepOn());
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
  // Refill pump: looked at only, never saved nor tested (a Test runs a pump).
  check(
    'the Refill pump section shows its address and a Test button',
    (await click('button', 'Refill pump')) &&
      (await sleep(800),
      await js(`!!document.querySelector('input[type=number][max="247"]') && [...document.querySelectorAll('button')].some((b) => b.textContent.includes('Test (3 s)'))`)),
  );

  // --- Tubing calibration last: it is only looked at, its draft untouched
  await click('button', 'Pump control');
  await sleep(500);
  check('Tubing calibration opens', await click('.tab', 'Tubing calibration'));
  await sleep(1800);
  check('the calibration shows its four steps', (await js(`document.querySelectorAll('.steps .step').length`)) === 4);
  // "burst" is the code's word; the page says measurement (2026-10-20)
  check('the calibration says measurement, never burst', await js(`!/burst/i.test(document.querySelector('main')?.innerText ?? document.body.innerText)`));
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
