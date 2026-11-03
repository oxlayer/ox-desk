// Casca nativa sobre o monitor.py. Nao duplica logica: o Python coleta e
// serve a UI, o Electron da janela, bandeja e atalhos.
const { app, BrowserWindow, Tray, Menu, shell, nativeImage } = require('electron');
const { spawn } = require('node:child_process');
const http = require('node:http');
const path = require('node:path');
const fs = require('node:fs');

const CFG = JSON.parse(
  fs.readFileSync(path.join(__dirname, '..', 'hosts.json'), 'utf8')
);
const PORT = CFG.port || 8787;
const URL = `http://127.0.0.1:${PORT}/`;
const PY = path.join(__dirname, '..', 'monitor.py');

let win = null;
let tray = null;
let child = null;      // backend que NOS subimos (se subimos)
let quitting = false;

// --- backend ---------------------------------------------------------------

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

async function ensureBackend() {
  // O systemd (lan-monitor.service) normalmente ja o tem no ar. So subimos
  // um filho se nao houver ninguem respondendo -- e so entao o matamos no fim.
  if (await ping()) return 'ja rodando';
  child = spawn('python3', [PY], { stdio: 'ignore', detached: false });
  let died = false;
  child.on('exit', () => { died = true; child = null; });
  for (let i = 0; i < 25; i++) {
    await new Promise((r) => setTimeout(r, 200));
    if (await ping(400)) {
      // Se o filho morreu mas a porta responde, quem atende e outro
      // (tipicamente o lan-monitor.service). Nao reivindicar o credito.
      return died ? 'ja rodando (nosso filho saiu: porta ocupada)' : 'subimos agora';
    }
  }
  return 'nao respondeu';
}

// --- janela ----------------------------------------------------------------

function createWindow() {
  win = new BrowserWindow({
    width: 1000,
    height: 780,
    minWidth: 380,
    minHeight: 460,
    title: 'Maquinas',
    icon: path.join(__dirname, 'icon.png'),
    backgroundColor: '#0c0e12',
    autoHideMenuBar: true,
    webPreferences: {
      // A pagina e local e nao recebe nada nosso: sem node, com isolamento.
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
    },
  });

  win.loadURL(URL);

  // Link externo abre no navegador, nunca dentro do app.
  win.webContents.setWindowOpenHandler(({ url }) => {
    shell.openExternal(url);
    return { action: 'deny' };
  });

  // Fechar esconde na bandeja; sair e pelo menu da bandeja.
  win.on('close', (e) => {
    if (!quitting) { e.preventDefault(); win.hide(); }
  });
}

function toggle() {
  if (!win) return createWindow();
  if (win.isVisible() && win.isFocused()) win.hide();
  else { win.show(); win.focus(); }
}

// --- bandeja ---------------------------------------------------------------

function createTray() {
  const img = nativeImage.createFromPath(path.join(__dirname, 'tray.png'));
  tray = new Tray(img.resize({ width: 22, height: 22 }));
  tray.setToolTip('Maquinas - painel da LAN');
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: 'Mostrar / esconder', click: toggle },
    { label: 'Recarregar', click: () => win && win.reload() },
    { type: 'separator' },
    { label: 'Abrir no navegador', click: () => shell.openExternal(URL) },
    { type: 'separator' },
    { label: 'Sair', click: () => { quitting = true; app.quit(); } },
  ]));
  tray.on('click', toggle);
}

// --- ciclo de vida ---------------------------------------------------------

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', () => { if (win) { win.show(); win.focus(); } });

  app.whenReady().then(async () => {
    const how = await ensureBackend();
    console.log(`backend em ${URL}: ${how}`);
    createWindow();
    createTray();
  });

  app.on('window-all-closed', () => { /* fica na bandeja */ });

  app.on('before-quit', () => { quitting = true; });

  // Mata o backend so se o filho for nosso; o do systemd segue vivo.
  app.on('will-quit', () => { if (child) child.kill(); });
}
