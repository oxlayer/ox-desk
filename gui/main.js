// Casca nativa sobre o logid. Nao duplica logica: o daemon coleta e serve
// a UI, o Electron da janela, bandeja e atalhos.
//
// Roda nas duas maquinas. O daemon e o mesmo binario nos dois SOs (ver
// specs/001), entao a casca tambem tem de ser -- a alternativa seria a
// GUI existir so no Ubuntu, que foi exatamente a assimetria reclamada.
// O que muda por SO sao caminhos, e so eles ficam em `paths()`.
const { app, BrowserWindow, Tray, Menu, shell, nativeImage } = require('electron');
const { spawn } = require('node:child_process');
const http = require('node:http');
const path = require('node:path');
const fs = require('node:fs');

// Os dois caminhos que mudam por SO. Tem de bater com o que o daemon usa
// -- `sys::config_dir` e onde o install poe o binario. Divergir aqui
// significa a GUI ler a config errada e nao achar o daemon.
function paths() {
  if (process.platform === 'win32') {
    const appdata = process.env.APPDATA || process.env.USERPROFILE || '.';
    const local = process.env.LOCALAPPDATA || appdata;
    return {
      config: path.join(appdata, 'logi', 'config.toml'),
      logid: path.join(local, 'logid', 'logid.exe'),
    };
  }
  const home = process.env.HOME || '/tmp';
  return {
    config: path.join(
      process.env.XDG_CONFIG_HOME || path.join(home, '.config'),
      'logi', 'config.toml'
    ),
    logid: path.join(home, '.local', 'bin', 'logid'),
  };
}

const PATHS = paths();

// A porta vem da config do logid (TOML). Regex em vez de parser: e uma
// linha escalar num arquivo que nos mesmos geramos.
function readPort() {
  try {
    const m = fs.readFileSync(PATHS.config, 'utf8').match(/^\s*port\s*=\s*(\d+)/m);
    if (m) return parseInt(m[1], 10);
  } catch { /* sem config ainda: o logid grava o default no primeiro boot */ }
  return 8787;
}

const PORT = readPort();
const URL = `http://127.0.0.1:${PORT}/`;
const LOGID = PATHS.logid;

let win = null;
let tray = null;
let child = null;      // backend que NOS subimos (se subimos)
let quitting = false;

// --- backend ---------------------------------------------------------------

/// GET + JSON do logid. Devolve null em qualquer falha: a bandeja
/// degrada para "sem dados" em vez de quebrar.
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

async function ensureBackend() {
  // O systemd (logid.service) normalmente ja o tem no ar. So subimos um
  // filho se nao houver ninguem respondendo -- e so entao o matamos no fim.
  if (await ping()) return 'ja rodando';
  const bin = fs.existsSync(LOGID) ? LOGID : 'logid';
  // windowsHide: o daemon e console app, e sem isto uma janela preta
  // pisca na tela a cada vez que a GUI o sobe.
  child = spawn(bin, [], { stdio: 'ignore', detached: false, windowsHide: true });
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

/// Linha de bateria de um dispositivo, para o menu.
function batteryLabel(d) {
  const parts = [];
  if (d.battery_percent != null) parts.push(d.battery_percent + '%');
  // O status vem do dispositivo e pode contradizer a realidade: medido,
  // um MX Keys no cabo subiu de 5% para 50% dizendo "descarregando".
  // Marcamos de quem e a afirmacao.
  if (d.charging) parts.push('carregando');
  else if (d.battery_status) parts.push(`diz: ${d.battery_status}`);
  if (d.battery_next_level != null) parts.push(`prox ${d.battery_next_level}%`);
  // Idade da leitura: com o dispositivo ausente, isto e memoria, e dizer
  // "5% (2min)" vale mais que "?" -- mas fingir que e agora seria pior.
  if (d.battery_age_s > 60) {
    const m = Math.round(d.battery_age_s / 60);
    parts.push(m >= 60 ? `ha ${Math.round(m / 60)}h` : `ha ${m}min`);
  }
  const bat = parts.length ? parts.join(' ') : 'bateria ?';
  const onde = d.online ? ('canal ' + (d.current_host ?? '?')) : 'em outra maquina';
  return `${d.name} - ${bat} - ${onde}`;
}

/// Manda o foco para um host. `fast` usa o cache de pares: a bandeja e
/// caminho interativo, nao vale esperar SSH.
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
  const [devs, cfg] = await Promise.all([
    getJson('/api/devices'),
    getJson('/api/config'),
  ]);

  const items = [];
  const list = (devs?.receivers ?? []).flatMap((r) => r.devices ?? []);

  if (devs?.error) {
    items.push({ label: devs.error.slice(0, 60), enabled: false });
  } else if (!list.length) {
    items.push({ label: 'nenhum dispositivo conhecido', enabled: false });
  } else {
    for (const d of list) items.push({ label: batteryLabel(d), enabled: false });
  }

  // Troca rapida: um item por host remoto declarado na config.
  const remotes = (cfg?.hosts ?? []).filter(
    (h) => !['127.0.0.1', 'localhost', '::1'].includes(h.addr)
  );
  const aqui = (cfg?.hosts ?? []).find(
    (h) => ['127.0.0.1', 'localhost', '::1'].includes(h.addr)
  );
  if (remotes.length) {
    items.push({ type: 'separator' });
    for (const h of remotes) {
      items.push({ label: `Mandar para ${h.name}`, click: () => focus(h.name) });
    }
    // Trazer de volta so faz sentido se algum dispositivo esta fora.
    if (aqui && list.some((d) => !d.online)) {
      items.push({ label: `Trazer para ${aqui.name}`, click: () => focus(aqui.name) });
    }
  }

  items.push(
    { type: 'separator' },
    { label: 'Mostrar / esconder', click: toggle },
    { label: 'Recarregar', click: () => win && win.reload() },
    { label: 'Abrir no navegador', click: () => shell.openExternal(URL) },
    { type: 'separator' },
    { label: 'Sair', click: () => { quitting = true; app.quit(); } }
  );

  tray.setContextMenu(Menu.buildFromTemplate(items));

  // Tooltip resume o que importa sem abrir o menu.
  const low = list.filter((d) => (d.battery_percent ?? 100) <= 15);
  const resumo = list.length
    ? list.map((d) => `${d.name}: ${d.battery_percent ?? '?'}%`).join(' | ')
    : 'sem dispositivos';
  tray.setToolTip(
    'logid' + (low.length ? '  [BATERIA BAIXA]' : '') + '\n' + resumo
  );
}

// --- recarregar quando o daemon mudar de build ----------------------------
//
// A pagina tambem sabe se recarregar, mas isso nao basta: o codigo que
// recarrega VIAJA NA PAGINA. Uma janela aberta antes de o recurso existir
// nunca o recebe -- foi o que aconteceu, e o usuario ficou olhando um
// painel de ontem enquanto o daemon estava quinze builds adiante, com
// razao de achar que o binario estava velho.
//
// Aqui, no processo principal, o relogio nao depende do que a pagina
// carregou: ele e dono da janela e recarrega do servidor.
let buildVisto = null;
async function vigiaBuild() {
  const h = await getJson('/healthz', 3000);
  if (!h || !h.build) return;                 // daemon reiniciando
  const atual = `${h.n ?? '?'} ${h.build}`;
  if (buildVisto === null) { buildVisto = atual; return; }
  if (atual === buildVisto) return;
  buildVisto = atual;                         // marca ANTES, para nao virar laco
  if (win && !win.isDestroyed()) {
    // Ignorando cache: a pagina vem com `no-store`, mas um recarregamento
    // que revalida ainda poderia servir o mesmo HTML de um proxy local.
    win.webContents.reloadIgnoringCache();
  }
}

function createTray() {
  const img = nativeImage.createFromPath(path.join(__dirname, 'tray.png'));
  tray = new Tray(img.resize({ width: 22, height: 22 }));
  tray.setToolTip('logid');
  tray.on('click', toggle);
  refreshTray();
  // A bandeja e lida de relance: manter fresca sem o usuario pedir.
  setInterval(refreshTray, 15000);
  // Mesma ideia para o build: a janela nao pode ficar atras do daemon.
  vigiaBuild();
  setInterval(vigiaBuild, 5000);
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
