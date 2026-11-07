// Casca nativa sobre o logid. Nao duplica logica: o daemon coleta e serve
// a UI, o Electron da janela, bandeja e atalhos.
//
// Roda nas duas maquinas. O daemon e o mesmo binario nos dois SOs (ver
// specs/001), entao a casca tambem tem de ser -- a alternativa seria a
// GUI existir so no Ubuntu, que foi exatamente a assimetria reclamada.
// O que muda por SO sao caminhos, e so eles ficam em `paths()`.
const { app, BrowserWindow, Tray, Menu, shell, nativeImage } = require('electron');

/// No Linux a bandeja e nativa (ksni dentro do logid). Windows e macOS
/// seguem com a do Electron ate ganharem a sua.
const BANDEJA_NATIVA = process.platform === 'linux';
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

/// Caminho de uma URL nossa, sem usar a classe `URL`.
///
/// `const URL = 'http://127.0.0.1:...'` ali em cima **sombreia** a
/// classe global com mesmo nome: `new URL(x)` aqui dentro morre com
/// "URL is not a constructor". Foi o que aconteceu, e o erro virou
/// `UnhandledPromiseRejectionWarning` no log -- a janela abria, so a
/// tela cheia nunca acontecia. Sintoma mudo, causa a dois arquivos de
/// distancia do nome.
///
/// `globalThis.URL` resolveria, mas uma funcao nomeada deixa o motivo
/// escrito onde a proxima pessoa vai olhar.
function caminhoDe(u) {
  return new globalThis.URL(String(u)).pathname;
}

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

// Quem supervisiona o daemon e o SO -- systemd no Linux, launchd no
// macOS, schtasks no Windows. A GUI pede ao supervisor; ela nao e um
// segundo supervisor.
//
// Isto nao e estilo: subir um filho nosso cria um SEGUNDO dono da porta.
// No boot do Mac a GUI ganhou a corrida contra o LaunchAgent, o filho
// ficou orfao (o launchd nao o conhece, `launchctl print` dizia
// `state = spawn scheduled`) e a copia supervisionada passou o dia
// morrendo em loop com `Address already in use`. Resultado: o deploy
// instalava o binario novo, dizia `ok` em todos os passos, e a maquina
// seguia servindo um build de dez dias atras. Quem notou foi o usuario.
//
// Pior ainda para o OTA: ele reinicia pelo launchd, pergunta o
// `/healthz`, ouve o build velho do orfao e REVERTE uma atualizacao que
// estava correta.
function comandoDoSupervisor() {
  if (process.platform === 'darwin') {
    return ['launchctl', ['kickstart', `gui/${process.getuid()}/ai.logi.logid`]];
  }
  if (process.platform === 'linux') {
    return ['systemctl', ['--user', 'start', 'logid.service']];
  }
  return ['schtasks', ['/run', '/tn', 'logid']];
}

function pedir(cmd, args) {
  return new Promise((resolve) => {
    const p = spawn(cmd, args, { stdio: 'ignore', windowsHide: true });
    p.on('error', () => resolve(false));
    p.on('exit', (code) => resolve(code === 0));
  });
}

async function esperarPorta(tentativas) {
  for (let i = 0; i < tentativas; i++) {
    await new Promise((r) => setTimeout(r, 200));
    if (await ping(400)) return true;
  }
  return false;
}

async function ensureBackend() {
  if (await ping()) return 'ja rodando';

  // Primeiro o supervisor. Se ele sobe o daemon, quem fica no ar e a
  // copia que o deploy e o OTA sabem reiniciar.
  if (await pedir(...comandoDoSupervisor())) {
    if (await esperarPorta(25)) return 'o supervisor subiu';
  }

  // Sem supervisor instalado (maquina nova, antes do assistente) a GUI
  // ainda tem de abrir -- e justamente pela GUI que se chega ao
  // assistente que instala o supervisor. Ai sim, filho avulso, e matado
  // no `will-quit`.
  const bin = fs.existsSync(LOGID) ? LOGID : 'logid';
  // windowsHide: o daemon e console app, e sem isto uma janela preta
  // pisca na tela a cada vez que a GUI o sobe.
  child = spawn(bin, [], { stdio: 'ignore', detached: false, windowsHide: true });
  let died = false;
  child.on('exit', () => { died = true; child = null; });
  if (await esperarPorta(25)) {
    // Se o filho morreu mas a porta responde, quem atende e outro.
    // Nao reivindicar o credito.
    return died ? 'ja rodando (nosso filho saiu: porta ocupada)' : 'subimos agora (sem supervisor)';
  }
  return 'nao respondeu';
}

// --- janela ----------------------------------------------------------------

function createWindow(rota = '/') {
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

  win.loadURL(URL.slice(0, -1) + rota);

  // Tela cheia so na configuracao, e so enquanto ela estiver aberta.
  //
  // Quem decide e a ROTA, por `did-navigate`, e nao a chamada: o link
  // "painel" no pe da pagina e a saida natural, e ele teria de tirar a
  // tela cheia por conta propria -- coisa que uma pagina nao pode
  // fazer. Amarrando no evento, voltar para `/` devolve a janela
  // normal sozinho, e ir para `/setup` pelo menu da bandeja entra em
  // tela cheia sem caminho especial.
  //
  // Tela cheia sem saida visivel seria sequestro de tela; a pagina
  // sempre mostra o link do painel, e o Esc continua valendo.
  const ajustarTela = (url) => {
    const cheia = caminhoDe(url).startsWith('/setup');
    if (win.isFullScreen() === cheia) return;
    win.setFullScreen(cheia);
    // Uma linha no log por troca: sem ela, "a tela cheia nao
    // aconteceu" nao se distingue de "a navegacao nao chegou aqui" --
    // e a primeira versao disto falhava calada, por sombreamento de
    // nome.
    console.log(`janela: ${cheia ? 'tela cheia' : 'normal'} (${caminhoDe(url)})`);
  };
  win.webContents.on('did-navigate', (_e, url) => ajustarTela(url));
  ajustarTela(URL.slice(0, -1) + rota);

  // Link externo abre no navegador, nunca dentro do app.
  win.webContents.setWindowOpenHandler(({ url }) => {
    shell.openExternal(url);
    return { action: 'deny' };
  });

  // Fechar esconde na bandeja -- mas so onde ELA existe.
  //
  // No Linux a bandeja agora e nativa, dentro do logid, e esta casca
  // nao cria mais a dela. Esconder a janela sem bandeja propria
  // deixaria um processo invisivel e sem caminho de volta: o menu
  // nativo abre o painel no navegador, nao esta janela. Entao aqui
  // fechar e fechar.
  win.on('close', (e) => {
    if (!quitting && !BANDEJA_NATIVA) { e.preventDefault(); win.hide(); }
  });
}

/// Abre uma rota do painel na janela, criando-a se preciso.
function abrir(rota) {
  if (!win || win.isDestroyed()) return createWindow(rota);
  win.loadURL(URL.slice(0, -1) + rota);
  win.show();
  win.focus();
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
  const [devs, cfg, kvm] = await Promise.all([
    getJson('/api/devices'),
    getJson('/api/config'),
    getJson('/api/kvm'),
  ]);
  const porLan = kvm?.mode === 'lan';

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
      const ativo = porLan && kvm?.target === h.name;
      items.push({
        label: (ativo ? '\u25cf ' : '') + `Mandar para ${h.name}`,
        click: () => focus(h.name),
      });
    }
    // Quando mostrar "trazer de volta" depende do MECANISMO.
    //
    // No HID++ o dispositivo troca de canal e some do nosso receptor,
    // entao "algum dispositivo offline" e o sinal certo. No modo LAN
    // ele nunca sai daqui -- continua online -- e esse teste nunca
    // seria verdadeiro: o item sumiria justamente no modo em que ele e
    // o unico caminho de volta pelo menu.
    const fora = porLan ? Boolean(kvm?.target) : list.some((d) => !d.online);
    if (aqui && fora) {
      items.push({ label: `Trazer para ${aqui.name}`, click: () => focus(aqui.name) });
    }
  }

  items.push(
    { type: 'separator' },
    { label: 'Mostrar / esconder', click: toggle },
    { label: 'Configurar esta maquina', click: () => abrir('/setup') },
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

/// Mede o setup e leva para a configuracao se faltar algo local.
async function conferirSetup() {
  // Se o daemon nao responder, nao abre nada: um wizard que aparece
  // porque a medicao falhou e pior que nenhum wizard.
  const d = await getJson('/api/setup', 90000);
  if (!d || !d.passos) return;
  const local = d.passos.filter(
    (p) => p.essencial && p.id !== 'pares' && p.estado !== 'ok' && p.estado !== 'fora'
  );
  if (!local.length) return;
  console.log(`setup: falta ${local.map((p) => p.id).join(', ')} -- abrindo a configuracao`);
  // So se a pessoa ainda estiver na pagina inicial: se ela ja navegou,
  // mandar a janela para outro lugar seria tirar a tela da mao dela.
  if (!win || win.isDestroyed()) return;
  const onde = caminhoDe(win.webContents.getURL() || URL);
  if (onde !== '/' && onde !== '/index.html') return;
  abrir('/setup');
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
    // `--setup` abre direto na configuracao. Serve para quem chamou o
    // app de proposito para configurar, e e como esta tela se testa
    // sem depender de clicar na bandeja.
    const direto = process.argv.includes('--setup');
    createWindow(direto ? '/setup' : '/');
    if (direto) return;
    // A configuracao se abre sozinha quando falta algo nesta maquina --
    // DEPOIS da janela, nunca antes.
    //
    // `/api/setup` mede SSH para cada par e leva segundos; esperar por
    // ela para so entao abrir a janela faria o app parecer travado toda
    // vez. A janela nasce no painel e troca se for o caso.
    //
    // E `pares` NAO conta para abrir sozinho. Ele fala das OUTRAS
    // maquinas: com o MacBook dormindo, uma maquina perfeitamente
    // montada cairia na tela de configuracao a cada login -- e a
    // segunda vez que isso acontece a pessoa ja aprendeu a fechar sem
    // ler. Falta de verdade e o que esta errado AQUI.
    conferirSetup();
    // Bandeja so onde nao ha nativa. No Linux o logid registra a dele
    // por StatusNotifierItem; criar a do Electron junto poria DOIS
    // icones iguais na barra, cada um com um menu que o outro nao
    // atualiza.
    if (!BANDEJA_NATIVA) createTray();
  });

  // Sem bandeja propria, fechar a ultima janela tem de encerrar: um
  // Electron invisivel sem icone nenhum e um processo que so o
  // gerenciador de tarefas acha.
  app.on('window-all-closed', () => {
    if (BANDEJA_NATIVA) app.quit();
  });

  app.on('before-quit', () => { quitting = true; });

  // Mata o backend so se o filho for nosso; o do systemd segue vivo.
  app.on('will-quit', () => { if (child) child.kill(); });
}
