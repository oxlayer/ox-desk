# 016 — The home screen and the session

> Status: home screen, session in a tab, bar and file manager live. Quality
> control and monitor switching are still missing.

## The problem

The panel answers "what is up", but it does not look like a product: the
main action, opening a machine's screen, competes for space with ports, RAM
and options. And after connecting there is nothing besides the video: to
exchange a file you have to leave the session.

Parsec and AnyDesk are a reference for **organization**, not for branding:
the visual identity remains ours.

## What changes

### 1. Home screen

- One card per machine: name, system, state and **"Conectar"** (Connect)
  as the main button.
- Ports, RAM, CPU and services go into a detail that expands.
- Only the options the client knows how to fulfill appear.
- Search only from five machines up. With three, it is noise.

### 2. The session is a tab

Connecting opens a tab in the panel window, not another window. Several
machines stay open side by side, and the home screen is the first tab.

The bar sits at the top of the tab, and must not steal clicks from the
remote machine.

| Button | Does |
|---|---|
| "Arquivos" (Files) | opens the file manager on this machine |
| "Tela" (Screen) | picks the monitor, when there is more than one |
| "Qualidade" (Quality) | changes the `--mbps` ceiling |
| "Teclado" (Keyboard) | sends Ctrl+Alt+Del and system shortcuts |
| "Sair" (Exit) | ends the session |

### 3. File manager

Two panes: this machine on the left, the remote one on the right, with
path, back, up, reload, and the name, size and date columns. Upload and
download show progress and errors per file.

## How

**The video is decoded in the page.** `logistream-view` is another process
with a native window, and that cannot be embedded in a tab. A local bridge
speaks QUIC to the host and hands H.264 to the page over WebSocket on
`127.0.0.1`, with a token and an `Origin` check. The page decodes with
WebCodecs and draws on a `<canvas>`; keyboard and mouse take the reverse
path.

**Decoding is in software.** The stream does not declare that it does not
reorder, and the hardware decoder holds about 4 frames: 172 ms versus
1.9 ms (p50, 1080p, on the Mac). To go back to hardware, the host needs to
send `max_num_reorder_frames=0` in the H.264 VUI.

Measured on 2026-11-07, with host and client on the same Mac, at 3360×2100:
from capture to draw, 6.3 ms in the tab and 37 ms in `logistream-view`.
Decoding at that size costs 5.2 ms and about half a core. Between machines
the measurement carries the clock difference, which is why the comparison is
on the same Mac.

With this, tab, bar and files are the same page, and the bar is HTML.
`logistream-view` stays as an alternative until the tab's latency is
measured: it only replaces the native one if it is not worse.

**The file manager is a tab in the same way.** See [spec 003](003-files.md).

**Transfer goes over SFTP**, like the listing. The new route is
`POST /api/files/transfer`, with progress read by the panel.

`logid` keeps listening only on `127.0.0.1` (spec 008). It is the one doing
the transfer, with the key it already has.

## Rules this inherits

- **The proof of a screen is looking at the image.** Counting frames proves
  nothing ([[../docs/screen-paths]]).
- **A click on the bar does not go to the remote machine.** The test checks
  that no `Evento::Mouse` went out.
- **The tab is measured against `logistream-view`:** the delay from capture
  to screen.
- **Transfer is proven by the hash on both sides**, not by the progress bar
  finishing ([[../.claude/rules/verify-what-arrived]]).
- **The Windows path is the SFTP one** (`/C:/Users/...`), which already has
  conversion in `files.rs`.

## Order

1. Home screen, without touching the API. It is just `static/index.html`.
2. Bridge and session tab, with latency measured against the native one.
3. Session bar, with the shortcuts.
4. File manager and the transfer route.
5. Quality and monitor switching, which depend on changing the host
   mid-session.

## Out of scope here

- Pairing by key for the screen channel (spec 012).
- Remote audio, chat and session recording.
