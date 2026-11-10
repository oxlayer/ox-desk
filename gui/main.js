// Native shell over oxdesk. It duplicates no logic: the daemon collects data
// and serves the UI, Electron provides the window, tray and shortcuts.
//
// Runs on both machines. The daemon is the same binary on both OSes (see
// specs/001), so the shell has to be as well -- the alternative was a GUI
// that only existed on Ubuntu, which was exactly the asymmetry people
// complained about. What changes per OS is paths, and only those live in
// `paths()`.
const { app, BrowserWindow, Tray, Menu, shell, nativeImage } = require('electron');

/// On Linux the tray is native (ksni inside oxdesk). Windows and macOS keep
/// Electron's tray until they get their own.
const NATIVE_TRAY = process.platform === 'linux';
const { spawn } = require('node:child_process');
const http = require('node:http');
const path = require('node:path');
const fs = require('node:fs');

// The two paths that change per OS. They must match what the daemon uses
// -- `sys::config_dir` and where the installer puts the binary. Diverging
// here means the GUI reads the wrong config and cannot find the daemon.
function paths() {
  if (process.platform === 'win32') {
    const appdata = process.env.APPDATA || process.env.USERPROFILE || '.';
    const local = process.env.LOCALAPPDATA || appdata;
    return {
      config: path.join(appdata, 'oxdesk', 'config.toml'),
      oxdesk: path.join(local, 'oxdesk', 'oxdesk.exe'),
    };
  }
  const home = process.env.HOME || '/tmp';
  return {
    config: path.join(
      process.env.XDG_CONFIG_HOME || path.join(home, '.config'),
      'oxdesk', 'config.toml'
    ),
    oxdesk: path.join(home, '.local', 'bin', 'oxdesk'),
  };
}

const PATHS = paths();

// The port comes from oxdesk's config (TOML). A regex instead of a parser:
// it is one scalar line in a file we generate ourselves.
function readPort() {
  try {
    const m = fs.readFileSync(PATHS.config, 'utf8').match(/^\s*port\s*=\s*(\d+)/m);
    if (m) return parseInt(m[1], 10);
  } catch { /* no config yet: oxdesk writes the default on first boot */ }
  return 8787;
}

const PORT = readPort();
const URL = `http://127.0.0.1:${PORT}/`;
const DAEMON_BIN = PATHS.oxdesk;

/// Path of one of our URLs, without using the `URL` class.
///
/// `const URL = 'http://127.0.0.1:...'` above **shadows** the global
/// class of the same name: `new URL(x)` in here dies with "URL is not a
/// constructor". That is what happened, and the error became an
/// `UnhandledPromiseRejectionWarning` in the log -- the window opened,
/// only full screen never happened. A silent symptom, with the cause two
/// files away from the name.
///
/// `globalThis.URL` would solve it, but a named function leaves the reason
/// written where the next person will look.
function pathOf(u) {
  return new globalThis.URL(String(u)).pathname;
}

let win = null;
let tray = null;
let child = null;      // backend that WE started (if we did)
let quitting = false;

// --- backend ---------------------------------------------------------------

/// GET + JSON from oxdesk. Returns null on any failure: the tray degrades
/// to "no data" instead of breaking.
function getJson(path, timeout = 4000) {
  return new Promise((resolve) => {
    const req = http.get(
      { host: '127.0.0.1', port: PORT, path, timeout },
      (res) => {
        if (res.statusCode !== 200) { res.resume(); return resolve(null); }
        let body = '';
        res.setEncoding('utf8');
        res.on('data', (c) => { body += c; });
        res.on('end', () => {
          try { resolve(JSON.parse(body)); } catch { resolve(null); }
        });
      }
    );
    req.on('error', () => resolve(null));
    req.on('timeout', () => { req.destroy(); resolve(null); });
  });
}

function ping(timeout = 800) {
  return new Promise((resolve) => {
    const req = http.get(
      { host: '127.0.0.1', port: PORT, path: '/healthz', timeout },
      (res) => { res.resume(); resolve(res.statusCode === 200); }
    );
    req.on('error', () => resolve(false));
    req.on('timeout', () => { req.destroy(); resolve(false); });
  });
}

// The OS supervises the daemon -- systemd on Linux, launchd on macOS,
// schtasks on Windows. The GUI asks the supervisor; it is not a second
// supervisor.
//
// This is not style: spawning a child of our own creates a SECOND owner of
// the port. At Mac boot the GUI won the race against the LaunchAgent, the
// child was orphaned (launchd does not know it, `launchctl print` said
// `state = spawn scheduled`) and the supervised copy spent the day dying in
// a loop with `Address already in use`. Result: deploy installed the new
// binary, said `ok` at every step, and the machine kept serving a build
// from ten days earlier. The user was the one who noticed.
//
// Worse for OTA: it restarts through launchd, asks `/healthz`, hears the
// orphan's old build and REVERTS an update that was correct.
function supervisorCommand() {
  if (process.platform === 'darwin') {
    return ['launchctl', ['kickstart', `gui/${process.getuid()}/app.oxdesk.daemon`]];
  }
  if (process.platform === 'linux') {
    return ['systemctl', ['--user', 'start', 'oxdesk.service']];
  }
  return ['schtasks', ['/run', '/tn', 'oxdesk']];
}

function request(cmd, args) {
  return new Promise((resolve) => {
    const p = spawn(cmd, args, { stdio: 'ignore', windowsHide: true });
    p.on('error', () => resolve(false));
    p.on('exit', (code) => resolve(code === 0));
  });
}

async function waitForPort(attempts) {
  for (let i = 0; i < attempts; i++) {
    await new Promise((r) => setTimeout(r, 200));
    if (await ping(400)) return true;
  }
  return false;
}

async function ensureBackend() {
  if (await ping()) return 'already running';

  // Supervisor first. If it starts the daemon, the copy left running is
  // the one deploy and OTA know how to restart.
  if (await request(...supervisorCommand())) {
    if (await waitForPort(25)) return 'started by the supervisor';
  }

  // With no supervisor installed (new machine, before the wizard) the GUI
  // still has to open -- the GUI is precisely how one reaches the wizard
  // that installs the supervisor. Only then a standalone child, killed
  // on `will-quit`.
  const bin = fs.existsSync(DAEMON_BIN) ? DAEMON_BIN : 'oxdesk';
  // windowsHide: the daemon is a console app, and without this a black
  // window flashes on screen every time the GUI starts it.
  child = spawn(bin, [], { stdio: 'ignore', detached: false, windowsHide: true });
  let died = false;
  child.on('exit', () => { died = true; child = null; });
  if (await waitForPort(25)) {
    // If the child died but the port answers, someone else is serving.
    // Do not claim the credit.
    return died ? 'already running (our child exited: port in use)' : 'started now (no supervisor)';
  }
  return 'did not respond';
}

// --- window ----------------------------------------------------------------

function createWindow(route = '/') {
  win = new BrowserWindow({
    width: 1000,
    height: 780,
    minWidth: 380,
    minHeight: 460,
    title: 'Machines',
    icon: path.join(__dirname, 'icon.png'),
    backgroundColor: '#0c0e12',
    autoHideMenuBar: true,
    webPreferences: {
      // The page is local and receives nothing of ours: no node, isolated.
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
    },
  });

  win.loadURL(URL.slice(0, -1) + route);

  // Full screen only on the setup page, and only while it is open.
  //
  // The ROUTE decides, via `did-navigate`, not the call: the "panel" link
  // at the bottom of the page is the natural exit, and it would have to
  // leave full screen on its own -- something a page cannot do. Tied to
  // the event, going back to `/` restores the normal window by itself, and
  // going to `/setup` from the tray menu enters full screen with no
  // special path.
  //
  // Full screen with no visible exit would be hijacking the screen; the
  // page always shows the panel link, and Esc still works.
  const adjustScreen = (url) => {
    const full = pathOf(url).startsWith('/setup');
    if (win.isFullScreen() === full) return;
    win.setFullScreen(full);
    // One log line per switch: without it, "full screen did not happen"
    // is indistinguishable from "navigation never got here" -- and the
    // first version of this failed silently, through name shadowing.
    console.log(`window: ${full ? 'full screen' : 'normal'} (${pathOf(url)})`);
  };
  win.webContents.on('did-navigate', (_e, url) => adjustScreen(url));
  adjustScreen(URL.slice(0, -1) + route);

  // External links open in the browser, never inside the app.
  win.webContents.setWindowOpenHandler(({ url }) => {
    shell.openExternal(url);
    return { action: 'deny' };
  });

  // Closing hides to the tray -- but only where THAT tray exists.
  //
  // On Linux the tray is now native, inside oxdesk, and this shell no
  // longer creates its own. Hiding the window without a tray of our own
  // would leave an invisible process with no way back: the native menu
  // opens the panel in the browser, not this window. So here closing
  // means closing.
  win.on('close', (e) => {
    if (!quitting && !NATIVE_TRAY) { e.preventDefault(); win.hide(); }
  });
}

/// Opens a panel route in the window, creating it if needed.
function open(route) {
  if (!win || win.isDestroyed()) return createWindow(route);
  win.loadURL(URL.slice(0, -1) + route);
  win.show();
  win.focus();
}

function toggle() {
  if (!win) return createWindow();
  if (win.isVisible() && win.isFocused()) win.hide();
  else { win.show(); win.focus(); }
}

// --- tray ------------------------------------------------------------------

/// Battery line of a device, for the menu.
function batteryLabel(d) {
  const parts = [];
  if (d.battery_percent != null) parts.push(d.battery_percent + '%');
  // The status comes from the device and can contradict reality: measured,
  // an MX Keys on the cable went from 5% to 50% saying "discharging". We
  // mark whose claim it is.
  if (d.charging) parts.push('charging');
  else if (d.battery_status) parts.push(`says: ${d.battery_status}`);
  if (d.battery_next_level != null) parts.push(`next ${d.battery_next_level}%`);
  // Age of the reading: with the device absent this is memory, and saying
  // "5% (2min)" is worth more than "?" -- but pretending it is current
  // would be worse.
  if (d.battery_age_s > 60) {
    const m = Math.round(d.battery_age_s / 60);
    parts.push(m >= 60 ? `${Math.round(m / 60)}h ago` : `${m}min ago`);
  }
  const bat = parts.length ? parts.join(' ') : 'battery ?';
  const where = d.online ? ('channel ' + (d.current_host ?? '?')) : 'on another machine';
  return `${d.name} - ${bat} - ${where}`;
}

/// Sends focus to a host. `fast` uses the peer cache: the tray is an
/// interactive path, not worth waiting on SSH.
async function focus(host) {
  const body = JSON.stringify({ host, confirm: true, fast: true });
  await new Promise((resolve) => {
    const req = http.request(
      { host: '127.0.0.1', port: PORT, path: '/api/focus', method: 'POST',
        headers: { 'Content-Type': 'application/json',
                   'Content-Length': Buffer.byteLength(body) },
        timeout: 20000 },
      (res) => { res.resume(); res.on('end', resolve); }
    );
    req.on('error', resolve);
    req.on('timeout', () => { req.destroy(); resolve(); });
    req.end(body);
  });
  await refreshTray();
}

async function refreshTray() {
  if (!tray) return;
  const [devs, cfg, kvm] = await Promise.all([
    getJson('/api/devices'),
    getJson('/api/config'),
    getJson('/api/kvm'),
  ]);
  const viaLan = kvm?.mode === 'lan';

  const items = [];
  const list = (devs?.receivers ?? []).flatMap((r) => r.devices ?? []);

  if (devs?.error) {
    items.push({ label: devs.error.slice(0, 60), enabled: false });
  } else if (!list.length) {
    items.push({ label: 'no known device', enabled: false });
  } else {
    for (const d of list) items.push({ label: batteryLabel(d), enabled: false });
  }

  // Quick switch: one item per remote host declared in the config.
  const remotes = (cfg?.hosts ?? []).filter(
    (h) => !['127.0.0.1', 'localhost', '::1'].includes(h.addr)
  );
  const here = (cfg?.hosts ?? []).find(
    (h) => ['127.0.0.1', 'localhost', '::1'].includes(h.addr)
  );
  if (remotes.length) {
    items.push({ type: 'separator' });
    for (const h of remotes) {
      const active = viaLan && kvm?.target === h.name;
      items.push({
        label: (active ? '● ' : '') + `Send to ${h.name}`,
        click: () => focus(h.name),
      });
    }
    // When to show "bring back" depends on the MECHANISM.
    //
    // With HID++ the device switches channel and disappears from our
    // receiver, so "some device offline" is the right signal. In LAN mode
    // it never leaves here -- it stays online -- and that test would never
    // be true: the item would vanish precisely in the mode where it is the
    // only way back through the menu.
    const away = viaLan ? Boolean(kvm?.target) : list.some((d) => !d.online);
    if (here && away) {
      items.push({ label: `Bring it to ${here.name}`, click: () => focus(here.name) });
    }
  }

  items.push(
    { type: 'separator' },
    { label: 'Show / hide', click: toggle },
    { label: 'Set up this machine', click: () => open('/setup') },
    { label: 'Reload', click: () => win && win.reload() },
    { label: 'Open in the browser', click: () => shell.openExternal(URL) },
    { type: 'separator' },
    { label: 'Quit', click: () => { quitting = true; app.quit(); } }
  );

  tray.setContextMenu(Menu.buildFromTemplate(items));

  // The tooltip summarizes what matters without opening the menu.
  const low = list.filter((d) => (d.battery_percent ?? 100) <= 15);
  const summary = list.length
    ? list.map((d) => `${d.name}: ${d.battery_percent ?? '?'}%`).join(' | ')
    : 'no devices';
  tray.setToolTip(
    'oxdesk' + (low.length ? '  [BATERIA BAIXA]' : '') + '\n' + summary
  );
}

// --- reload when the daemon changes build ---------------------------------
//
// The page also knows how to reload itself, but that is not enough: the
// code that reloads TRAVELS IN THE PAGE. A window opened before the feature
// existed never receives it -- that is what happened, and the user kept
// looking at yesterday's panel while the daemon was fifteen builds ahead,
// rightly thinking the binary was stale.
//
// Here, in the main process, the clock does not depend on what the page
// loaded: it owns the window and reloads from the server.
let seenBuild = null;
async function watchBuild() {
  const h = await getJson('/healthz', 3000);
  if (!h || !h.build) return;                 // daemon restarting
  const current = `${h.n ?? '?'} ${h.build}`;
  if (seenBuild === null) { seenBuild = current; return; }
  if (current === seenBuild) return;
  seenBuild = current;                        // mark FIRST, so it cannot loop
  if (win && !win.isDestroyed()) {
    // Ignoring cache: the page comes with `no-store`, but a revalidating
    // reload could still serve the same HTML from a local proxy.
    win.webContents.reloadIgnoringCache();
  }
}

/// Measures the setup and goes to the setup page if something local is missing.
async function checkSetup() {
  // If the daemon does not answer, open nothing: a wizard that shows up
  // because the measurement failed is worse than no wizard.
  const d = await getJson('/api/setup', 90000);
  if (!d || !d.steps) return;
  const local = d.steps.filter(
    (p) => p.essential && p.id !== 'peers' && p.state !== 'ok' && p.state !== 'not_applicable'
  );
  if (!local.length) return;
  console.log(`setup: missing ${local.map((p) => p.id).join(', ')} -- opening the setup page`);
  // Only if the person is still on the home page: if they already
  // navigated, moving the window elsewhere would take the screen out of
  // their hands.
  if (!win || win.isDestroyed()) return;
  const where = pathOf(win.webContents.getURL() || URL);
  if (where !== '/' && where !== '/index.html') return;
  open('/setup');
}

function createTray() {
  const img = nativeImage.createFromPath(path.join(__dirname, 'tray.png'));
  tray = new Tray(img.resize({ width: 22, height: 22 }));
  tray.setToolTip('oxdesk');
  tray.on('click', toggle);
  refreshTray();
  // The tray is read at a glance: keep it fresh without the user asking.
  setInterval(refreshTray, 15000);
  // Same idea for the build: the window cannot lag behind the daemon.
  watchBuild();
  setInterval(watchBuild, 5000);
}

// --- lifecycle -------------------------------------------------------------

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', () => { if (win) { win.show(); win.focus(); } });

  app.whenReady().then(async () => {
    const how = await ensureBackend();
    console.log(`backend at ${URL}: ${how}`);
    // `--setup` opens straight on the setup page. It is for whoever
    // launched the app on purpose to configure it, and it is how this
    // screen gets tested without clicking the tray.
    const direct = process.argv.includes('--setup');
    createWindow(direct ? '/setup' : '/');
    if (direct) return;
    // The setup page opens by itself when something is missing on this
    // machine -- AFTER the window, never before.
    //
    // `/api/setup` measures SSH to every peer and takes seconds; waiting
    // for it before opening the window would make the app look frozen
    // every time. The window is born on the panel and switches if needed.
    //
    // And `peers` does NOT count for opening on its own. It is about the
    // OTHER machines: with the MacBook asleep, a perfectly set-up machine
    // would land on the setup page at every login -- and by the second
    // time the person has learned to close it without reading. A real
    // gap is what is wrong HERE.
    checkSetup();
    // A tray only where there is no native one. On Linux oxdesk registers
    // its own via StatusNotifierItem; creating Electron's as well would put
    // TWO identical icons in the bar, each with a menu the other does not
    // update.
    if (!NATIVE_TRAY) createTray();
  });

  // Without a tray of our own, closing the last window must quit: an
  // invisible Electron with no icon at all is a process only the task
  // manager finds.
  app.on('window-all-closed', () => {
    if (NATIVE_TRAY) app.quit();
  });

  app.on('before-quit', () => { quitting = true; });

  // Kill the backend only if the child is ours; systemd's stays alive.
  app.on('will-quit', () => { if (child) child.kill(); });
}
