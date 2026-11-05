// Teste de fumaca da pagina de configuracao (`/setup`).
//
// Mesmo motivo do check-ui.js: "o JS compila" nao e "a pagina funciona".
// Esta pagina em especial desenha a lista inteira por JS, a partir de um
// JSON que muda de forma por sistema operacional -- um campo com outro
// nome nao da erro de sintaxe, da uma linha em branco na tela de quem
// esta tentando montar a maquina.
//
//   cd gui && env -u ELECTRON_RUN_AS_NODE \
//     ./node_modules/electron/dist/electron check-setup.js
//
// O `env -u` importa: com `ELECTRON_RUN_AS_NODE=1` no ambiente, o
// binario do Electron roda como Node puro e `require('electron')`
// devolve undefined.
const { app, BrowserWindow } = require('electron');
const http = require('node:http');
const erros = [];

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
    const nivel = (a && a.level !== undefined) ? a.level : b;
    const msg = (a && a.message !== undefined) ? a.message : c;
    if (nivel === 'error' || nivel === 'warning' || Number(nivel) >= 2) erros.push(nivel + ': ' + msg);
  });
  win.webContents.on('render-process-gone', (_e, d) => erros.push('processo morreu: ' + JSON.stringify(d)));

  // A VERDADE vem da API, nao da pagina: o teste compara o que o
  // daemon mediu com o que a tela mostra. Sem isso ele diria "10
  // linhas, otimo" sobre dez linhas erradas.
  const api = await getJson('/api/setup');
  if (!api) { console.log('ERROS ["/api/setup nao respondeu"]'); app.exit(1); return; }

  await win.loadURL('http://127.0.0.1:8787/setup');
  // A medicao do lado do daemon abre SSH para cada par: pode levar
  // segundos, e a pagina so desenha quando ela volta.
  await new Promise((r) => setTimeout(r, 45000));

  const estado = () => win.webContents.executeJavaScript(`(() => ({
    titulo: document.querySelector('#maquina').textContent,
    lead: document.querySelector('#lead').textContent,
    linhas: document.querySelectorAll('#lista li').length,
    ids: [...document.querySelectorAll('#lista li')].map(li =>
      li.querySelector('.tit .n') ? li.querySelector('.tit .n').textContent : '?'),
    vazias: [...document.querySelectorAll('#lista li .tit .d')].filter(d => !d.textContent.trim()).length,
    ok: document.querySelectorAll('#lista li.ok').length,
    agora: document.querySelectorAll('#lista li.agora').length,
    abertos: document.querySelectorAll('#lista li.aberto').length,
    botoes: document.querySelectorAll('#lista button').length,
    barra: document.querySelector('#barra').style.width,
    prontos: document.querySelector('#prontos').textContent,
    total: document.querySelector('#total').textContent,
    hora: document.querySelector('#hora').textContent,
    rolagemHorizontal: document.documentElement.scrollWidth > window.innerWidth + 1
  }))()`);

  const r = await estado();
  console.log('ESTADO ' + JSON.stringify(r));

  // --- conferencias contra a API ---------------------------------------
  const esperado = api.passos.length;
  if (r.linhas !== esperado) erros.push(`a pagina desenhou ${r.linhas} passos e a API mandou ${esperado}`);
  if (r.vazias) erros.push(`${r.vazias} passo(s) sem detalhe na tela`);
  if (Number(r.prontos) !== api.resumo.prontos) erros.push('contador de prontos divergente');
  if (Number(r.total) !== api.resumo.total) erros.push('contador de total divergente');
  // A hora e so plausibilidade, NAO igualdade: o teste mede uma vez e a
  // pagina mede de novo segundos depois, entao os relogios diferem de
  // direito. A primeira versao desta linha comparava as duas e acusava
  // um defeito que nao existia.
  if (!/^\d{2}:\d{2}:\d{2}$/.test(r.hora)) erros.push('a hora da medicao nao chegou na tela: ' + r.hora);
  // Os contadores saem da mesma medicao da pagina, entao ESSES tem de
  // bater com a API a menos que algo tenha mudado no meio.

  // Um passo pendente SEM destaque nem acao e uma tela sem saida: a
  // pessoa le "falta isso" e nao tem o que clicar.
  if (api.resumo.proximo && r.agora !== 1) erros.push(`a API aponta ${api.resumo.proximo} e a tela destacou ${r.agora}`);
  if (api.resumo.proximo && r.abertos !== 1) erros.push('o proximo passo nao abriu sozinho');
  if (api.resumo.proximo && !r.botoes) erros.push('passo pendente aberto e nenhum botao na tela');
  if (r.rolagemHorizontal) erros.push('rolagem horizontal na largura cheia');

  // --- abrir e fechar na mao -------------------------------------------
  // Abrir um passo na mao tem de FECHAR o anterior: dois corpos abertos
  // ao mesmo tempo empurram a lista e tiram do campo de visao o passo
  // que o wizard estava indicando.
  await win.webContents.executeJavaScript(`document.querySelector('#lista li .head').click()`);
  await new Promise((r) => setTimeout(r, 300));
  const clicado = await win.webContents.executeJavaScript(
    `document.querySelectorAll('#lista li.aberto').length`);
  console.log('APOS_CLICAR ' + JSON.stringify({ abertos: clicado }));
  if (clicado > 1) erros.push(`${clicado} passos abertos ao mesmo tempo`);

  // --- janela estreita --------------------------------------------------
  win.setSize(420, 820);
  await new Promise((r) => setTimeout(r, 600));
  const estreito = await win.webContents.executeJavaScript(`(() => ({
    rolagemHorizontal: document.documentElement.scrollWidth > window.innerWidth + 1,
    largura: Math.round(document.querySelector('.wrap').getBoundingClientRect().width)
  }))()`);
  console.log('ESTREITO ' + JSON.stringify(estreito));
  if (estreito.rolagemHorizontal) erros.push('rolagem horizontal em 420 px');

  console.log('ERROS ' + JSON.stringify(erros));
  app.exit(erros.length ? 1 : 0);
});
