// Smoke test of the UI served by a running daemon, in a real (headless)
// browser: the paths that broke once, walked end to end. It changes nothing:
// no run or burst is started, the calibration draft is only looked at, no
// setting is saved, and the browser profile is a throwaway one.
//
//   npm run smoke                       (daemon on http://127.0.0.1:8730)
//   SMOKE_URL=http://127.0.0.1:8731 npm run smoke
//   SMOKE_CDP=9444 npm run smoke        (the desktop window instead of Edge)
//
// SMOKE_CDP walks the compiled desktop window, started with
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9444: its
// page is http://tauri.localhost, the daemon still SMOKE_URL.
//
// Needs Edge or Chrome. Exits 1 on the first failed check, or on any page
// error (an exception in the page fails the run even if every check passed).
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const URL = process.env.SMOKE_URL ?? 'http://127.0.0.1:8730';
const CDP = process.env.SMOKE_CDP ? Number(process.env.SMOKE_CDP) : null;
// where the pages are: the daemon itself, or the desktop window's bundle
const APP = CDP ? 'http://tauri.localhost' : URL;
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
const exe = CDP ? 'desktop window' : BROWSERS.find((p) => existsSync(p));
if (!exe) {
  console.error('FAIL  no Edge or Chrome found');
  process.exit(1);
}
const port = CDP ?? 9300 + Math.floor(Math.random() * 500);
const profile = CDP ? null : mkdtempSync(join(tmpdir(), 'ft-smoke-'));
// the desktop window is the user's: attached to, never started nor closed
const browser = CDP
  ? { kill() {} }
  : spawn(exe, ['--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, '--window-size=1400,1000', 'about:blank'], { stdio: 'ignore' });
let ws;
function finish(code) {
  try { ws?.close(); } catch { /* closing anyway */ }
  // Edge's children (GPU, renderers) outlive a kill of the parent alone and
  // held its profile and memory (run 256): the whole tree on Windows.
  if (process.platform === 'win32' && browser.pid) {
    try { spawnSync('taskkill', ['/F', '/T', '/PID', String(browser.pid)], { stdio: 'ignore' }); } catch { /* gone already */ }
  }
  browser.kill();
  setTimeout(() => {
    try { if (profile) rmSync(profile, { recursive: true, force: true }); } catch { /* the browser may hold it a moment */ }
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
  await cmd('Page.navigate', { url: APP });
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
    check('a fresh New run has no green step', (await greens()) === '-----', await greens());
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
    // No regulation: the setpoint goes to the pump as typed, no tube is asked
    const trimWasOn = await js(`[...document.querySelectorAll('[aria-label="Regulation"] button')].some((b) => b.textContent.includes('Balance') && b.getAttribute('aria-checked') === 'true')`);
    await click('[aria-label="Regulation"] button', 'No regulation');
    await sleep(300);
    check('No regulation asks for no tube', await js(`!document.querySelector('#nr-tube') && !!document.querySelector('#nr-raw')`));
    if (trimWasOn) {
      await click('[aria-label="Regulation"] button', 'Balance regulation');
      await sleep(300);
    }

    await click('.step', 'Feed profile');
    await sleep(400);
    await type('input[aria-label="Hours"]', '2');
    await sleep(900);
    check('Feed profile turns green with a duration', (await greens())[2] === 'G', await greens());
    check('the curve is drawn beside the profile', await js(`!!document.querySelector('.preview svg')`));
    // a time scale under it, in minutes for this 2 h curve (hours past 2 h)
    check('the curve has a time scale, 0 to 120 min', await js(`(() => { const t = [...document.querySelectorAll('.preview svg text')].map((e) => e.textContent.trim()); return t.includes('0') && t.includes('120 min'); })()`));

    // Steps: typed in a table, a step back in time is pointed at (the 4th
    // before the 3rd), the file buttons are there. Nothing is imported.
    check('the Steps shape opens a table of one step', (await click('.shape', 'Steps')) && (await sleep(300), await js(`document.querySelectorAll('#nr-steps tbody tr').length === 1`)));
    for (let i = 0; i < 3; i++) await click('#nr-steps button', 'Add a step');
    await type('input[aria-label="Step 2 hours"]', '0.5');
    await type('input[aria-label="Step 3 hours"]', '1');
    await type('input[aria-label="Step 4 hours"]', '0.25');
    await sleep(400);
    check('a step typed back in time is refused, on its row', await js(`(() => { const r = document.querySelectorAll('#nr-steps tbody tr'); return r.length === 5 && r[3].classList.contains('bad-row') && r[4]?.classList.contains('why-row') && /Step 4 starts at or before step 3/.test(r[4].textContent); })()`));
    check('Feed profile is not green with a step back in time', (await greens())[2] === '-', await greens());
    await type('input[aria-label="Step 4 hours"]', '1.5');
    await sleep(900);
    check('steps in order draw the curve', await js(`!document.querySelector('#nr-steps .bad-row') && !!document.querySelector('.preview svg')`));
    check("each step draws its stretch on the run's time", await js(`document.querySelectorAll('#nr-steps .track i').length === 4`));
    check('Steps has Import from Excel and Template', await js(`(() => { const b = [...document.querySelectorAll('#nr-steps button')].map((x) => x.textContent.trim()); return b.includes('Import from Excel') && b.includes('Template'); })()`));
    check("the run's end is the last row of the steps", await js(`document.querySelector('#nr-steps .end input')?.value === '2' && !document.querySelector('input[aria-label="Hours"]')`));
    check('Clear all steps waits in the "…" menu', (await click('#nr-steps button[aria-label="More step actions"]')) && (await sleep(200), await js(`[...document.querySelectorAll('#nr-steps .menu button')].some((b) => b.textContent.trim() === 'Clear all steps')`)));
    await js(`document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);
    await click('.shape', 'Linear');
    await sleep(900);
    // the Timelapse step (2026-11-06): its switch and interval buttons with a camera, the way to Settings without
    await click('.step', 'Timelapse');
    await sleep(600);
    check('Timelapse turns green once seen', (await greens())[3] === 'G', await greens());
    check('the Timelapse step has its switch or the way to Settings', await js(`!!document.querySelector('.tl-switch input[type=checkbox]') || !!document.querySelector('.panel a[href="#settings/camera"]')`));
    if (await js(`!!document.querySelector('[aria-label="One image every"]')`)) {
      await click('[aria-label="One image every"] button', 'Custom');
      await sleep(300);
      check('Custom opens its minutes field', await js(`!!document.querySelector('.tl-custom input')`));
      await click('[aria-label="One image every"] button', '5 min');
      await sleep(300);
      check('a preset closes the Custom field', await js(`!document.querySelector('.tl-custom input')`));
    }
    // where the images go, said before Start (2026-11-18): the folder, Browse… (never clicked: it opens Windows' picker), the run's own folder
    if (await js(`!!document.querySelector('.tl-switch input:checked')`)) {
      check('the Timelapse step says where the images go, with Browse', await js(`(() => { const w = document.querySelector('.tl-where'); return !!w && /\\S/.test(w.querySelector('.path')?.textContent ?? '') && [...w.querySelectorAll('button')].some((b) => b.textContent.trim() === 'Browse…') && /a folder of their own: \\d{4}-\\d{2}-\\d{2}_\\d{4} /.test(w.textContent); })()`));
    }
    await click('.step', 'Check and start');
    await sleep(600);
    check('Check and start shows the curve and a Start button', await js(`!!document.querySelector('.go .preview svg') && [...document.querySelectorAll('button')].some((b) => b.textContent.includes('Start run'))`));
    check('Check and start sums the timelapse up in one line', await js(`(() => { const dt = [...document.querySelectorAll('.recap dt')].find((d) => d.textContent.trim() === 'Timelapse'); const t = dt?.nextElementSibling?.textContent.trim() ?? ''; return /^(every \\d|off$|no camera$)/.test(t); })()`));
    if (await js(`/^every/.test([...document.querySelectorAll('.recap dt')].find((d) => d.textContent.trim() === 'Timelapse')?.nextElementSibling?.textContent.trim() ?? '')`)) {
      check('Check and start says the folder the images go in', await js(`/_\\d{4} /.test(document.querySelector('.recap .tl-dest')?.textContent ?? '')`));
    }

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
      // the two stacked charts start their plot at the same x: their times line up
      const plotEdges = await js(`[...document.querySelectorAll('.modal.detail .block svg')].map((s) => s.querySelector('text.tick')?.getAttribute('x')).filter((x) => x != null).slice(0, 2)`);
      if (plotEdges.length >= 2) check('the feed rate and running total charts line up', new Set(plotEdges).size === 1, plotEdges);
      // the desktop window's smallest size, 900 x 600 (2026-11-18): the
      // detail takes the height, its sticky head a quarter of it at most
      await cmd('Emulation.setDeviceMetricsOverride', { width: 900, height: 600, deviceScaleFactor: 1, mobile: false });
      await sleep(500);
      const short = await js(`(() => { const m = document.querySelector('.modal.detail').getBoundingClientRect(); const h = document.querySelector('.modal.detail .detail-head').getBoundingClientRect(); return { m: Math.round(m.height), h: Math.round(h.height) }; })()`);
      check('at 900 x 600, the run detail uses the window and keeps its head small', short.m >= 560 && short.h <= short.m / 4, JSON.stringify(short));
      await cmd('Emulation.clearDeviceMetricsOverride');
      await sleep(300);
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

    // --- History: a run's images, Play and Open folder (2026-11-18)
    const runsList = await (await fetch(`${URL}/api/runs?limit=40`)).json();
    let withImages = null;
    for (const r of runsList.filter((x) => x.kind !== 'calibration' && x.status !== 'running')) {
      const d = await (await fetch(`${URL}/api/runs/${r.id}`)).json();
      if (d.timelapse?.images > 1 && d.timelapse.folder_exists) {
        withImages = d;
        break;
      }
    }
    if (withImages) {
      await click('.tab', 'History');
      await sleep(1500);
      const rowOpened = await js(`(() => { const b = [...document.querySelectorAll('button.tr')].find((x) => x.querySelector('.rid')?.textContent.trim() === '#${withImages.id}'); if (!b) return false; b.click(); return true; })()`);
      if (rowOpened) {
        await sleep(2500);
        check('a run with a timelapse shows its Images, Play and Open folder', await js(`(() => { const s = document.querySelector('.modal.detail section.images'); const b = s ? [...s.querySelectorAll('button')].map((x) => x.textContent.trim()) : []; return b.includes('Play') && b.includes('Open folder'); })()`));
        check('its pictures are spread over the run, the first at its start', await js(`(() => { const t = [...document.querySelectorAll('.modal.detail .images .thumb span')].map((e) => e.textContent.trim()); return t.length >= 2 && t.length <= 6 && t[0] === '0 min'; })()`));
        await js(`[...document.querySelectorAll('.modal.detail .images button')].find((b) => b.textContent.trim() === 'Play').click()`);
        await sleep(800);
        check('Play opens the player on the first image', await js(`(() => { const p = document.querySelector('.player'); return !!p && /Image 1 of \\d+/.test(p.textContent) && !!p.querySelector('.stage img'); })()`));
        // the whole picture in the player, never cropped (2026-11-19: a max-height in the grid never applied, top and bottom cut)
        check('the player shows the whole picture, not cropped', await js(`(() => { const st = document.querySelector('.player .stage'); const im = st?.querySelector('img'); if (!im) return false; const a = st.getBoundingClientRect(); const b = im.getBoundingClientRect(); return b.height <= a.height + 1 && b.width <= a.width + 1 && getComputedStyle(im).objectFit === 'contain'; })()`));
        check('each image says when it was taken', await js(`/ \\d{4}, \\d{2}:\\d{2}:\\d{2}$/.test(document.querySelector('.player .when')?.textContent.trim() ?? '')`));
        await js(`document.querySelector('.player button[aria-label="Next image"]').click()`);
        await sleep(300);
        check('the next image button steps one image', await js(`/Image 2 of/.test(document.querySelector('.player').textContent)`));
        await js(`[...document.querySelectorAll('.player .speeds button')].find((b) => b.textContent.trim() === '50/s').click()`);
        await sleep(200);
        check('a speed says how long the whole run plays', await js(`/the whole run in \\d+ (s|min)/.test(document.querySelector('.player .whole')?.textContent ?? '')`));
        await js(`document.querySelector('.player .play').click()`);
        await sleep(1500);
        const at = await js(`Number(/Image (\\d+) of/.exec(document.querySelector('.player').textContent)?.[1])`);
        check('Play moves through the images', at > 2, String(at));
        await js(`document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);
        await sleep(400);
        check('Escape closes the player, the run detail stays', await js(`!document.querySelector('.player') && !!document.querySelector('.modal.detail')`));
      } else {
        console.log(`skip  Images: run ${withImages.id} is not in the list`);
      }
    } else {
      console.log('skip  Images: no run with a timelapse in History');
    }
  }

  // --- Settings, Display: shows its sample, nothing saved
  check('Settings opens', await click('button', 'Settings'));
  await sleep(800);
  check('the Display section shows a tube size', (await click('button', 'Display')) && (await sleep(800), await js(`!!document.querySelector('.sample .pri')`)));
  // The tint under a picked choice slides (lib/segthumb.js). Its observer
  // once reacted to its own class change, looped and froze the page: a page
  // that answers within 3 s, with the tint placed under the pick.
  const thumb = await Promise.race([
    js(`(() => { const s = document.querySelector('.settings-pane .seg'); return !!s && s.classList.contains('thumbed') && parseFloat(s.style.getPropertyValue('--thumb-w')) > 0; })()`),
    sleep(3000).then(() => 'frozen'),
  ]);
  check('a choice has its sliding tint, the page answering', thumb === true, String(thumb));
  // Pumps: the cable and every pump on it (2026-10-21). Looked at only:
  // never saved, reconnected nor tested (a Test runs a pump).
  check('the Pumps section opens', (await click('.side button', 'Pumps')) && (await sleep(1200), await js(`!!document.querySelector('.pumps')`)));
  check('it says the cable state first, in a word, above the port', await js(`(() => { const h = document.querySelector('.settings-pane .head .word'); const p = document.querySelector('.conn select.port'); return !!h && !!p && /^(Connected|Not connected|Not answering|Simulator)$/.test(h.textContent.trim()) && (h.compareDocumentPosition(p) & Node.DOCUMENT_POSITION_FOLLOWING) > 0; })()`));
  // Disconnect while the cable is up, Connect while it is down (never clicked here)
  // the state is a tinted band; Disconnect on it has no white box of its own (it sat on the band like a sticker)
  check('the state is a tinted band, Disconnect without a box on it', await js(`(() => { const h = document.querySelector('.settings-pane .head'); const d = h && [...h.querySelectorAll('button')].find((b) => b.textContent.trim() === 'Disconnect'); return !!h && getComputedStyle(h).backgroundColor !== 'rgba(0, 0, 0, 0)' && (!d || getComputedStyle(d).backgroundColor === 'rgba(0, 0, 0, 0)'); })()`));
  check('the cable button says Connect or Disconnect', await js(`(() => { const b = [...document.querySelectorAll('.head button')].map((x) => x.textContent.trim()); return b.includes('Connect') !== b.includes('Disconnect'); })()`));
  check('it lists the feed and the refill pump, each with an address', await js(`document.querySelectorAll('.pumps .item').length === 2 && document.querySelectorAll('.pumps .addr input').length === 2`));
  // the refill address is another pump: locked with its model (audit 2026-11-02)
  check('the refill address locks with the refill model', await js(`(() => { const a = document.querySelectorAll('.pumps .addr input')[1]; const m = document.querySelector('.pumps [aria-label="Refill pump model"] [role=radio]'); return !!a && !!m && a.disabled === m.disabled; })()`));
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
  check('no "Saved" before anything was saved', await js(`!/Saved/.test(document.querySelector('.settings-pane .save-state')?.textContent ?? '')`));

  // --- the desktop app's narrowest window, 900 px (UX audit, 2026-11-06):
  // sections laid out on the window's width squeezed a 350 px card
  await cmd('Emulation.setDeviceMetricsOverride', { width: 900, height: 800, deviceScaleFactor: 1, mobile: false });
  await sleep(600);
  const fits = () => js(`document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1`);
  check('at 900 px, Notifications keeps its names and topics readable', await js(`[...document.querySelectorAll('.settings-pane .row input')].every((i) => i.getBoundingClientRect().width >= 120)`));
  check('at 900 px, Pumps shows the three models of each pump', (await click('.side button', 'Pumps')) && (await sleep(1200), await js(`(() => { const pane = document.querySelector('.settings-pane').getBoundingClientRect(); const b = [...document.querySelectorAll('.settings-pane .seg button')]; return b.length >= 6 && b.every((x) => { const r = x.getBoundingClientRect(); return r.width > 30 && r.right <= pane.right + 1; }); })()`)));
  check('at 900 px, Settings fits the window', await fits());

  // --- Camera (2026-11-03): looked at only, never Connect, Disconnect nor
  // Save. Both reached by their deep link, as a bookmark would.
  const camCfg = async () => JSON.stringify((await (await fetch(`${URL}/api/config`)).json()).camera ?? null);
  const camBefore = await camCfg();
  const answers = (expr) => Promise.race([js(expr), sleep(3000).then(() => 'frozen')]);
  // A live view left must close its stream: the daemon's viewer count back
  // to what it was before (another open tab may hold one), within 5 s.
  const camViewers = async () => (await (await fetch(`${URL}/api/camera`)).json()).viewers;
  const viewersBefore = await camViewers();
  check('GET /api/camera counts the live viewers', Number.isInteger(viewersBefore), String(viewersBefore));
  const viewersBack = async () => {
    for (let i = 0; i < 25; i++) {
      if ((await camViewers()) <= viewersBefore) return true;
      await sleep(200);
    }
    return false;
  };
  await cmd('Page.navigate', { url: 'about:blank' });
  await sleep(300);
  await cmd('Page.navigate', { url: `${APP}/#settings/camera` });
  await sleep(2500);
  check('#settings/camera opens the Camera section', (await js(`document.querySelector('.side button.on')?.textContent.trim()`)) === 'Camera');
  check('it says the camera state in a word, with its Connect or Disconnect', await js(`/^(Connected|Connecting…|Camera in use|Camera not answering|Not connected|Camera helper missing)$/.test(document.querySelector('.settings-pane .head .word')?.textContent.trim() ?? '') && document.querySelectorAll('.settings-pane .head button').length === 1`));
  check('it has the camera select and the picture sizes', await js(`!!document.querySelector('.settings-pane select[aria-label="Camera"]') && document.querySelectorAll('.settings-pane .seg[aria-label="Picture size"] button').length >= 1`));
  // a webcam plugged in after a first list stayed out of the select until
  // "Look again": the section asks Windows again at each opening (2026-11-03)
  const plugged = (await (await fetch(`${URL}/api/camera/devices?refresh=1`)).json()).length;
  const offered = await js(`document.querySelectorAll('.settings-pane select[aria-label="Camera"] option[value]:not([value=""])').length`);
  check('the camera select lists every camera plugged in now', offered >= plugged, `${offered} offered, ${plugged} plugged`);
  // Every click there connects or sets the camera and saves (2026-11-05, no
  // Save): looked at only, nothing clicked.
  check('Camera has no Save button (each change applies and saves at once)', await js(`!document.querySelector('.settings-pane button.save')`));
  const placed = await answers(`(() => { const s = document.querySelector('.settings-pane .seg[aria-label="Picture size"]'); return !!s && s.classList.contains('thumbed') && parseFloat(s.style.getPropertyValue('--thumb-w')) > 0; })()`);
  check('the picture size choice answers, its tint placed', placed === true, String(placed));
  check('the live view offers 5, 10 and 15 a second, one picked', await js(`(() => { const b = [...document.querySelectorAll('.settings-pane .seg[aria-label="Live view, pictures a second"] button')]; return b.map((x) => x.textContent.trim()).join() === '5,10,15' && b.filter((x) => x.classList.contains('on')).length === 1; })()`));
  if ((await (await fetch(`${URL}/api/camera`)).json()).state === 'connected') {
    await sleep(1500);
    const ctl = await (await fetch(`${URL}/api/camera/controls`)).json();
    const rows = await js(`document.querySelectorAll('.settings-pane .ctls .ctl').length`);
    check('Picture shows one row per control the camera has', rows === (ctl.controls ?? []).length, `${rows} rows, ${(ctl.controls ?? []).length} controls`);
    check('each Picture row has its slider or its choice', await js(`[...document.querySelectorAll('.settings-pane .ctls .ctl')].every((r) => r.querySelector('input[type="range"], .seg') && r.querySelector('.name')?.textContent.trim())`));
  } else {
    console.log('skip  the Picture rows: no camera connected');
  }
  {
    const saved = JSON.parse(camBefore) ?? {};
    const shown = await js(`[document.querySelector('.settings-pane input[aria-label="Timelapse every (minutes)"]')?.value, document.querySelector('.settings-pane input[aria-label="Images saved in"]')?.value]`);
    check('Timelapse shows its interval and folder as saved', shown?.[0] === String(saved.interval_min ?? 5) && shown?.[1] === (saved.captures_dir ?? ''), JSON.stringify(shown));
  }
  check('nothing about the camera saved', (await camCfg()) === camBefore);
  await cmd('Page.navigate', { url: 'about:blank' });
  await sleep(300);
  check('leaving Settings, Camera closes its live preview (viewers back)', await viewersBack(), `viewers ${await camViewers()}, before ${viewersBefore}`);
  await cmd('Page.navigate', { url: `${APP}/#camera` });
  await sleep(2500);
  check('#camera opens the Camera page', (await js(`document.querySelector('.nav-item.active')?.textContent.trim()`)) === 'Camera' && (await js(`document.querySelector('h1')?.textContent.trim()`)) === 'Camera');
  const shown = await answers(`!!document.querySelector('.camera .pic, .camera .empty')`);
  check('the Camera page answers, with its picture or its empty state', shown === true, String(shown));
  if (await js(`!!document.querySelector('.camera .pic')`)) {
    await sleep(1500);
    check('the Timelapse card answers in one of its states', await js(`(() => { const c = document.querySelector('.camera section.tl'); return !!c && (!!c.querySelector('.rec .word') || [...c.querySelectorAll('button')].some((b) => b.textContent.includes('Start timelapse')) || /A run is going/.test(c.textContent)); })()`));
    check('Take a picture is offered beside Full screen', await js(`[...document.querySelectorAll('.camera .under button')].some((b) => b.textContent.includes('Take a picture'))`));
    // its icon sat on the words' baseline, touching them, then 1.5 px above
    // their lowercase middle (2026-11-16): a hair under the line box's middle
    check('the Take a picture icon is centred, apart from its words', await js(`(() => { const b = [...document.querySelectorAll('.camera .under button')].find((x) => x.textContent.includes('Take a picture')); const i = b?.querySelector('svg')?.getBoundingClientRect(); if (!i) return false; const r = b.getBoundingClientRect(); const n = [...b.childNodes].find((x) => x.nodeType === 3 && x.textContent.trim()); if (!n) return false; const t = document.createRange(); t.selectNode(n); const w = t.getBoundingClientRect(); const d = (i.top + i.bottom) / 2 - (w.top + w.bottom) / 2; return Math.abs((i.top + i.bottom) / 2 - (r.top + r.bottom) / 2) <= 2 && d >= 0.5 && d <= 2 && w.left - i.right >= 4; })()`));
  }
  check('the Camera page never says burst', await js(`!/burst/i.test(document.querySelector('main')?.innerText ?? '')`));
  // The desktop window's webview opens no new tab: an image or a web page
  // is opened by the daemon (2026-11-17), never by a target=_blank link.
  check('the Camera page has no link opening a new tab', await js(`!document.querySelector('main a[target="_blank"]')`));
  // Play on the images taken so far, during a run too (2026-11-19); Open folder is never clicked
  const curTl = (await (await fetch(`${URL}/api/timelapses/current`)).json()).timelapse;
  if (curTl?.images > 1) {
    const playBtn = await js(`(() => { const b = [...document.querySelectorAll('.strip-head button')].find((x) => x.textContent.trim() === 'Play'); if (!b) return false; b.click(); return true; })()`);
    check('Camera: Latest images has Play and Open folder', playBtn && await js(`[...document.querySelectorAll('.strip-head button')].some((x) => x.textContent.trim() === 'Open folder')`));
    await sleep(1200);
    check('Camera: Play opens the player on the images so far', await js(`/Image 1 of \\d+/.test(document.querySelector('.player')?.textContent ?? '')`));
    await js(`document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);
    await sleep(400);
    check('Camera: Escape closes the player', await js(`!document.querySelector('.player')`));
  } else {
    console.log('skip  Camera Play: no timelapse with images going');
  }
  await cmd('Page.navigate', { url: 'about:blank' });
  await sleep(300);
  await cmd('Page.navigate', { url: `${APP}/#settings/about` });
  await sleep(2500);
  check('About opens, its links open no new tab', await js(`document.querySelector('.side button.on')?.textContent.trim() === 'About' && !!document.querySelector('.settings-pane a[href^="https://"]') && !document.querySelector('a[target="_blank"]')`));

  // --- Tubing calibration last: it is only looked at, its draft untouched
  await click('button', 'Feeding profile');
  await sleep(500);
  check('leaving the Camera page closes its live view (viewers back)', await viewersBack(), `viewers ${await camViewers()}, before ${viewersBefore}`);
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

  await click('.tab', 'Tubing calibration');
  await sleep(800);
  check('at 900 px, Tubing calibration fits the window', await fits(), String(await js(`document.documentElement.scrollWidth`)));

  check('no error in the page', pageErrors.length === 0, pageErrors.join(' | '));
  console.log(`\n${passed} checks passed`);
  finish(0);
} catch (e) {
  if (e.message !== '__finished') {
    console.error(`FAIL  ${e.message}`);
    finish(1);
  }
}
