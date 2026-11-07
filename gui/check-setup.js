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
// The output markers (`ESTADO`, `APOS_CLICAR`, `ESTREITO`, `ERROS`) are
// kept as they are: the docs say "it passes when it prints `ERROS []`".
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
  if (!api) { console.log('ERROS ["/api/setup did not respond"]'); app.exit(1); return; }

  await win.loadURL('http://127.0.0.1:8787/setup');
  // The daemon-side measurement opens SSH to every peer: it can take
  // seconds, and the page only draws when it comes back.
  await new Promise((r) => setTimeout(r, 45000));

  const state = () => win.webContents.executeJavaScript(`(() => ({
    title: document.querySelector('#maquina').textContent,
    lead: document.querySelector('#lead').textContent,
    rows: document.querySelectorAll('#lista li').length,
    ids: [...document.querySelectorAll('#lista li')].map(li =>
      li.querySelector('.tit .n') ? li.querySelector('.tit .n').textContent : '?'),
    empty: [...document.querySelectorAll('#lista li .tit .d')].filter(d => !d.textContent.trim()).length,
    ok: document.querySelectorAll('#lista li.ok').length,
    now: document.querySelectorAll('#lista li.agora').length,
    open: document.querySelectorAll('#lista li.aberto').length,
    buttons: document.querySelectorAll('#lista button').length,
    bar: document.querySelector('#barra').style.width,
    ready: document.querySelector('#prontos').textContent,
    total: document.querySelector('#total').textContent,
    time: document.querySelector('#hora').textContent,
    horizontalScroll: document.documentElement.scrollWidth > window.innerWidth + 1
  }))()`);

  const r = await state();
  console.log('ESTADO ' + JSON.stringify(r));

  // --- checks against the API ------------------------------------------
  const expected = api.passos.length;
  if (r.rows !== expected) errors.push(`the page drew ${r.rows} steps and the API sent ${expected}`);
  if (r.empty) errors.push(`${r.empty} step(s) with no detail on screen`);
  if (Number(r.ready) !== api.resumo.prontos) errors.push('ready counter diverges');
  if (Number(r.total) !== api.resumo.total) errors.push('total counter diverges');
  // The time is only a plausibility check, NOT equality: the test measures
  // once and the page measures again seconds later, so the clocks rightly
  // differ. The first version of this line compared the two and reported a
  // defect that did not exist.
  if (!/^\d{2}:\d{2}:\d{2}$/.test(r.time)) errors.push('the measurement time did not reach the screen: ' + r.time);
  // The counters come from the page's own measurement, so THOSE must match
  // the API unless something changed in between.

  // A pending step with NO highlight and no action is a dead-end screen:
  // the person reads "this is missing" and has nothing to click.
  if (api.resumo.proximo && r.now !== 1) errors.push(`the API points to ${api.resumo.proximo} and the screen highlighted ${r.now}`);
  if (api.resumo.proximo && r.open !== 1) errors.push('the next step did not open by itself');
  if (api.resumo.proximo && !r.buttons) errors.push('pending step open and no button on screen');
  if (r.horizontalScroll) errors.push('horizontal scroll at full width');

  // --- open and close by hand ------------------------------------------
  // Opening a step by hand must CLOSE the previous one: two bodies open at
  // once push the list and take the step the wizard was pointing at out of
  // view.
  await win.webContents.executeJavaScript(`document.querySelector('#lista li .head').click()`);
  await new Promise((r) => setTimeout(r, 300));
  const clicked = await win.webContents.executeJavaScript(
    `document.querySelectorAll('#lista li.aberto').length`);
  console.log('APOS_CLICAR ' + JSON.stringify({ open: clicked }));
  if (clicked > 1) errors.push(`${clicked} steps open at the same time`);

  // --- narrow window ----------------------------------------------------
  win.setSize(420, 820);
  await new Promise((r) => setTimeout(r, 600));
  const narrow = await win.webContents.executeJavaScript(`(() => ({
    horizontalScroll: document.documentElement.scrollWidth > window.innerWidth + 1,
    width: Math.round(document.querySelector('.wrap').getBoundingClientRect().width)
  }))()`);
  console.log('ESTREITO ' + JSON.stringify(narrow));
  if (narrow.horizontalScroll) errors.push('horizontal scroll at 420 px');

  console.log('ERROS ' + JSON.stringify(errors));
  app.exit(errors.length ? 1 : 0);
});
