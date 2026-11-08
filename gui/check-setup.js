// Smoke test of the setup page (`/setup`).
//
// Same reason as check-ui.js: "the JS compiles" is not "the page works".
// This page in particular draws the whole list in JS, from a JSON whose
// shape changes per operating system -- a field with a different name is
// not a syntax error, it is a blank line on the screen of someone trying
// to set up the machine.
//
//   cd gui && env -u ELECTRON_RUN_AS_NODE \
//     ./node_modules/electron/dist/electron check-setup.js
//
// The `env -u` matters: with `ELECTRON_RUN_AS_NODE=1` in the environment,
// the Electron binary runs as plain Node and `require('electron')`
// returns undefined.
//
// The output markers (`STATE`, `AFTER_CLICK`, `NARROW`, `ERRORS`) are what
// the docs tell people to look for: it passes when it prints `ERRORS []`.
const { app, BrowserWindow } = require('electron');
const http = require('node:http');
const errors = [];

function getJson(path) {
  return new Promise((resolve) => {
    http.get({ host: '127.0.0.1', port: 8787, path, timeout: 60000 }, (res) => {
      let b = '';
      res.on('data', (c) => (b += c));
      res.on('end', () => { try { resolve(JSON.parse(b)); } catch (e) { resolve(null); } });
    }).on('error', () => resolve(null)).on('timeout', () => resolve(null));
  });
}

app.whenReady().then(async () => {
  const win = new BrowserWindow({ show: false, width: 1100, height: 900,
    webPreferences: { nodeIntegration: false, contextIsolation: true, sandbox: true } });
  win.webContents.on('console-message', (a, b, c) => {
    const level = (a && a.level !== undefined) ? a.level : b;
    const msg = (a && a.message !== undefined) ? a.message : c;
    if (level === 'error' || level === 'warning' || Number(level) >= 2) errors.push(level + ': ' + msg);
  });
  win.webContents.on('render-process-gone', (_e, d) => errors.push('render process died: ' + JSON.stringify(d)));

  // The TRUTH comes from the API, not from the page: the test compares what
  // the daemon measured with what the screen shows. Without that it would
  // say "10 lines, great" about ten wrong lines.
  const api = await getJson('/api/setup');
  if (!api) { console.log('ERRORS ["/api/setup did not respond"]'); app.exit(1); return; }

  await win.loadURL('http://127.0.0.1:8787/setup');
  // The daemon-side measurement opens SSH to every peer: it can take
  // seconds, and the page only draws when it comes back.
  await new Promise((r) => setTimeout(r, 45000));

  // Unfold every finished phase FIRST. A folded phase legitimately hides
  // its rows, so counting them against the API without this would report a
  // defect that is the feature working.
  await win.webContents.executeJavaScript(
    `[...document.querySelectorAll('#list li.phase.folded')].forEach(h => h.click()); true`);
  await new Promise((r) => setTimeout(r, 400));

  const state = () => win.webContents.executeJavaScript(`(() => ({
    title: document.querySelector('#machine').textContent,
    lead: document.querySelector('#lead').textContent,
    rows: document.querySelectorAll('#list li.step').length,
    phases: document.querySelectorAll('#list li.phase').length,
    folded: document.querySelectorAll('#list li.phase.folded').length,
    other: [...document.querySelectorAll('#list li.phase h2')]
      .filter(h => h.textContent === 'Other').length,
    ids: [...document.querySelectorAll('#list li.step')].map(li => li.dataset.id),
    empty: [...document.querySelectorAll('#list li.step .tit .state')]
      .filter(d => !d.textContent.trim()).length,
    now: document.querySelectorAll('#list li.now').length,
    open: document.querySelectorAll('#list li.open').length,
    buttons: document.querySelectorAll('#list button').length,
    bar: document.querySelector('#barfill').style.width,
    ready: document.querySelector('#done').textContent,
    total: document.querySelector('#total').textContent,
    time: document.querySelector('#clock').textContent,
    horizontalScroll: document.documentElement.scrollWidth > window.innerWidth + 1
  }))()`);

  const r = await state();
  console.log('STATE ' + JSON.stringify(r));

  // --- checks against the API ------------------------------------------
  const expected = api.steps.length;
  if (r.rows !== expected) errors.push(`the page drew ${r.rows} steps and the API sent ${expected}`);
  if (r.empty) errors.push(`${r.empty} step(s) with no state word on screen`);
  if (Number(r.ready) !== api.summary.done) errors.push('ready counter diverges');
  if (Number(r.total) !== api.summary.total) errors.push('total counter diverges');
  // Every step the daemon measures belongs to a named phase. A step landing
  // in "Other" is not a crash and not visible as one -- it is a row sitting
  // under a heading that tells the person nothing, which is how Windows'
  // `screen-task` went unnoticed. Catch it here.
  if (r.other) errors.push('a step fell into the "Other" phase: add its id to PHASES in setup.html');
  const drawn = new Set(r.ids);
  const unseen = api.steps.map((p) => p.id).filter((id) => !drawn.has(id));
  if (unseen.length) errors.push('the API sent steps the page never drew: ' + unseen.join(', '));
  // The time is only a plausibility check, NOT equality: the test measures
  // once and the page measures again seconds later, so the clocks rightly
  // differ. The first version of this line compared the two and reported a
  // defect that did not exist.
  if (!/^\d{2}:\d{2}:\d{2}$/.test(r.time)) errors.push('the measurement time did not reach the screen: ' + r.time);
  // The counters come from the page's own measurement, so THOSE must match
  // the API unless something changed in between.

  // A pending step with NO highlight and no action is a dead-end screen:
  // the person reads "this is missing" and has nothing to click.
  if (api.summary.next && r.now !== 1) errors.push(`the API points to ${api.summary.next} and the screen highlighted ${r.now}`);
  if (api.summary.next && r.open !== 1) errors.push('the next step did not open by itself');
  if (api.summary.next && !r.buttons) errors.push('pending step open and no button on screen');
  if (r.horizontalScroll) errors.push('horizontal scroll at full width');

  // --- open and close by hand ------------------------------------------
  // Opening a step by hand must CLOSE the previous one: two bodies open at
  // once push the list and take the step the wizard was pointing at out of
  // view.
  await win.webContents.executeJavaScript(`document.querySelector('#list li.step .head').click()`);
  await new Promise((r) => setTimeout(r, 300));
  const clicked = await win.webContents.executeJavaScript(
    `document.querySelectorAll('#list li.open').length`);
  console.log('AFTER_CLICK ' + JSON.stringify({ open: clicked }));
  if (clicked > 1) errors.push(`${clicked} steps open at the same time`);

  // --- narrow window ----------------------------------------------------
  win.setSize(420, 820);
  await new Promise((r) => setTimeout(r, 600));
  const narrow = await win.webContents.executeJavaScript(`(() => ({
    horizontalScroll: document.documentElement.scrollWidth > window.innerWidth + 1,
    width: Math.round(document.querySelector('.wrap').getBoundingClientRect().width)
  }))()`);
  console.log('NARROW ' + JSON.stringify(narrow));
  if (narrow.horizontalScroll) errors.push('horizontal scroll at 420 px');

  console.log('ERRORS ' + JSON.stringify(errors));
  app.exit(errors.length ? 1 : 0);
});
