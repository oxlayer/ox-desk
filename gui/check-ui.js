// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

// Smoke test of the panel: loads the page in a HIDDEN window, clicks what
// is clickable and reports whatever the console says.
//
// It exists because "the JS compiles" is not "the page works": a wrong id,
// a selector that returns null or a section that never shows up only
// surface at runtime -- and I do not see the user's screen.
//
//   cd gui && env -u ELECTRON_RUN_AS_NODE \
//     ./node_modules/electron/dist/electron check-ui.js
//
// The `env -u` matters: with `ELECTRON_RUN_AS_NODE=1` in the environment
// (the case here), the Electron binary runs as plain Node and
// `require('electron')` returns undefined.
//
// The output markers (`STATE`, `AFTER_LOG_CLICK`, `FAVORITE`, `NARROW`,
// `ERRORS`) are what the docs tell people to look for.
const { app, BrowserWindow } = require('electron');
const errors = [];
app.whenReady().then(async () => {
  const win = new BrowserWindow({ show: false, width: 1200, height: 820,
    webPreferences: { nodeIntegration: false, contextIsolation: true, sandbox: true } });
  // The signature changed between Electron versions: the new one passes an
  // object, the old one passes (event, level, message). Handling both keeps
  // the test from going blind exactly when we upgrade.
  win.webContents.on('console-message', (a, b, c) => {
    const level = (a && a.level !== undefined) ? a.level : b;
    const msg = (a && a.message !== undefined) ? a.message : c;
    const bad = level === 'error' || level === 'warning' || Number(level) >= 2;
    if (bad) errors.push(level + ': ' + msg);
  });
  win.webContents.on('render-process-gone', (_e, d) => errors.push('render process died: ' + JSON.stringify(d)));
  await win.loadURL('http://127.0.0.1:8787/');
  // Time for the initial fetches and renders to happen.
  await new Promise(r => setTimeout(r, 6000));
  const state = () => win.webContents.executeJavaScript(`(() => ({
    sections: document.querySelectorAll('.sec').length,
    visible: [...document.querySelectorAll('.sec')].filter(s => s.classList.contains('on')).map(s => s.dataset.sec),
    machines: document.querySelectorAll('#machinelist .navit').length,
    cards: document.querySelectorAll('#grid .card').length,
    screenButtons: document.querySelectorAll('button[data-open]').length,
    activeNav: [...document.querySelectorAll('.navit.on')].map(n => n.dataset.sec).filter(Boolean),
    sidebarWidth: Math.round(document.querySelector('.side').getBoundingClientRect().width),
    kvmPills: document.querySelectorAll('.u.warn, .units .u').length
  }))()`);

  const r = await state();
  console.log('STATE ' + JSON.stringify(r));

  // --- switch panel ----------------------------------------------------
  await win.webContents.executeJavaScript(
    `document.querySelector('.navit[data-sec="log"]').click()`);
  const onLog = await state();
  console.log('AFTER_LOG_CLICK ' + JSON.stringify(
    { visible: onLog.visible, activeNav: onLog.activeNav }));

  // --- favorite a machine ----------------------------------------------
  // Pin it, wait for `tick` to redraw, check it landed in Favorites.
  const name = await win.webContents.executeJavaScript(
    `document.querySelector('#machinelist .navit').dataset.maq`);
  await win.webContents.executeJavaScript(
    `document.querySelector('#machinelist button[data-fav]').click()`);
  await new Promise(r => setTimeout(r, 2500));
  const fav = await win.webContents.executeJavaScript(`(() => ({
    favorites: [...document.querySelectorAll('#favlist .navit')].map(n => n.dataset.maq),
    stored: JSON.parse(localStorage.getItem('favs') || '[]')
  }))()`);
  console.log('FAVORITO ' + JSON.stringify({ pinned: name, ...fav }));
  // Undo, so the next run starts clean.
  await win.webContents.executeJavaScript(`localStorage.removeItem('favs')`);

  // --- narrow window ----------------------------------------------------
  // The sidebar has to become a horizontal row, not disappear nor push the
  // content out: the Electron window opens small.
  win.setSize(700, 760);
  await new Promise(r => setTimeout(r, 600));
  const narrow = await win.webContents.executeJavaScript(`(() => {
    const s = document.querySelector('.side').getBoundingClientRect();
    return { sidebarWidth: Math.round(s.width), sidebarHeight: Math.round(s.height),
             horizontalScroll: document.documentElement.scrollWidth > window.innerWidth + 1 };
  })()`);
  console.log('NARROW ' + JSON.stringify(narrow));

  console.log('ERRORS ' + JSON.stringify(errors));
  app.exit(errors.length ? 1 : 0);
});
