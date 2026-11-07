// Electron launcher.
//
// It exists because of a gotcha we paid for: VS Code's extension host
// exports ELECTRON_RUN_AS_NODE=1, and with that variable in the environment
// electron starts as node and the window never appears. `npm start` used
// to run `env -u ELECTRON_RUN_AS_NODE electron .`, which is POSIX and does
// not run on Windows -- and the GUI has to run on both sides.
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
