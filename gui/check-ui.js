// Teste de fumaca do painel: carrega a pagina numa janela OCULTA, clica
// no que e clicavel e relata o que o console disser.
//
// Existe porque "o JS compila" nao e "a pagina funciona": um id errado,
// um seletor que devolve null ou uma secao que nunca aparece so se
// manifestam em runtime -- e eu nao vejo a tela do usuario.
//
//   cd gui && env -u ELECTRON_RUN_AS_NODE \
//     ./node_modules/electron/dist/electron check-ui.js
//
// O `env -u` importa: com `ELECTRON_RUN_AS_NODE=1` no ambiente (o caso
// aqui), o binario do Electron roda como Node puro e `require('electron')`
// devolve undefined.
// Carrega o painel numa janela OCULTA e relata o que o console disser.
// Existe porque "o JS compila" nao e "a pagina funciona": um id errado
// ou um seletor nulo so aparece em runtime.
const { app, BrowserWindow } = require('electron');
const erros = [];
app.whenReady().then(async () => {
  const win = new BrowserWindow({ show: false, width: 1200, height: 820,
    webPreferences: { nodeIntegration: false, contextIsolation: true, sandbox: true } });
  // A assinatura mudou entre versoes do Electron: a nova passa um
  // objeto, a antiga passa (evento, nivel, mensagem). Tratar as duas
  // evita que o teste fique cego justamente quando atualizarmos.
  win.webContents.on('console-message', (a, b, c) => {
    const nivel = (a && a.level !== undefined) ? a.level : b;
    const msg = (a && a.message !== undefined) ? a.message : c;
    const ruim = nivel === 'error' || nivel === 'warning' || Number(nivel) >= 2;
    if (ruim) erros.push(nivel + ': ' + msg);
  });
  win.webContents.on('render-process-gone', (_e, d) => erros.push('processo morreu: ' + JSON.stringify(d)));
  await win.loadURL('http://127.0.0.1:8787/');
  // Tempo para os fetches iniciais e os renders acontecerem.
  await new Promise(r => setTimeout(r, 6000));
  const estado = () => win.webContents.executeJavaScript(`(() => ({
    secoes: document.querySelectorAll('.sec').length,
    visiveis: [...document.querySelectorAll('.sec')].filter(s => s.classList.contains('on')).map(s => s.dataset.sec),
    maquinas: document.querySelectorAll('#maqlist .navit').length,
    cartoes: document.querySelectorAll('#grid .card').length,
    botoesTela: document.querySelectorAll('button[data-open]').length,
    navAtiva: [...document.querySelectorAll('.navit.on')].map(n => n.dataset.sec).filter(Boolean),
    larguraLateral: Math.round(document.querySelector('.side').getBoundingClientRect().width),
    kvmPills: document.querySelectorAll('.u.warn, .units .u').length
  }))()`);

  const r = await estado();
  console.log('ESTADO ' + JSON.stringify(r));

  // --- trocar de painel ------------------------------------------------
  await win.webContents.executeJavaScript(
    `document.querySelector('.navit[data-sec="log"]').click()`);
  const noLog = await estado();
  console.log('APOS_CLICAR_LOG ' + JSON.stringify(
    { visiveis: noLog.visiveis, navAtiva: noLog.navAtiva }));

  // --- favoritar uma maquina -------------------------------------------
  // Fixa, espera o `tick` redesenhar, confere que entrou em Favoritos.
  const nome = await win.webContents.executeJavaScript(
    `document.querySelector('#maqlist .navit').dataset.maq`);
  await win.webContents.executeJavaScript(
    `document.querySelector('#maqlist button[data-fav]').click()`);
  await new Promise(r => setTimeout(r, 2500));
  const fav = await win.webContents.executeJavaScript(`(() => ({
    favoritos: [...document.querySelectorAll('#favlist .navit')].map(n => n.dataset.maq),
    guardado: JSON.parse(localStorage.getItem('favs') || '[]')
  }))()`);
  console.log('FAVORITO ' + JSON.stringify({ fixei: nome, ...fav }));
  // Desfaz, para a proxima rodada comecar limpa.
  await win.webContents.executeJavaScript(`localStorage.removeItem('favs')`);

  // --- janela estreita --------------------------------------------------
  // A lateral tem de virar fila horizontal, nao sumir nem empurrar o
  // conteudo para fora: a janela do Electron abre pequena.
  win.setSize(700, 760);
  await new Promise(r => setTimeout(r, 600));
  const estreito = await win.webContents.executeJavaScript(`(() => {
    const s = document.querySelector('.side').getBoundingClientRect();
    return { lateralLargura: Math.round(s.width), lateralAltura: Math.round(s.height),
             rolagemHorizontal: document.documentElement.scrollWidth > window.innerWidth + 1 };
  })()`);
  console.log('ESTREITO ' + JSON.stringify(estreito));

  console.log('ERROS ' + JSON.stringify(erros));
  app.exit(erros.length ? 1 : 0);
});
