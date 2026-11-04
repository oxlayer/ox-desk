// Lancador do Electron.
//
// Existe por causa de uma pegadinha paga: o host de extensoes do VS Code
// exporta ELECTRON_RUN_AS_NODE=1, e com essa variavel no ambiente o
// electron sobe como node e a janela nunca aparece. O `npm start` fazia
// `env -u ELECTRON_RUN_AS_NODE electron .`, que e POSIX e nao roda no
// Windows -- e a GUI precisa rodar nos dois lados.
const { spawn } = require('node:child_process');

const env = { ...process.env };
delete env.ELECTRON_RUN_AS_NODE;

const electron = require('electron');
const bin = typeof electron === 'string' ? electron : electron.toString();

const child = spawn(bin, ['.', ...process.argv.slice(2)], {
  cwd: __dirname,
  env,
  stdio: 'inherit',
});
child.on('exit', (code) => process.exit(code ?? 0));
