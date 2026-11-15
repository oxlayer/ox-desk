# Deploy to the three machines

```bash
cd ~/dev/oxdesk && ./scripts/deploy.sh
```

Builds for Ubuntu, cross-compiles and signs for Windows, sends the
source to the Mac and builds there, restarts everything, and finishes by
checking that all three report the same build.

## Reading the output

The footer is what matters:

```
== check ==
  mac        #75 d72074b
  windows    #75 d72074b
  ubuntu     #75 d72074b

all three machines on the same build: d72074b
```

With `--skip-mac` the last line says `2 of 3 machines on the same build:
<build> (mac left out: --skip-mac)` instead, and that wording is
deliberate: claiming three when two were measured is exactly the kind of
silent gap the rules are about.

Any `OUT OF SYNC` means that machine fell behind, usually because a step
failed further up and its `ok` was false. Every Windows step captures
stderr and checks the size of the file that landed, precisely because
this has happened ([[../rules/verify-what-arrived]]).

## Don't edit the tree while the deploy runs

The stamp (`OXDESK_BUILD`) is taken **once, at the start**; the `rsync` to
the Mac happens **in the middle**. Editing a `.rs` between the two
produces two binaries with the **same stamp and different code** —
Ubuntu and Windows with the old file, the Mac with the new one, and the
footer check saying "three machines on the same build".

The stamp is then wrong, which is worse than visible divergence: the
panel detects divergence, and it cannot detect this.

This has happened. If the tree was edited mid-deploy, commit and run
again — with a clean tree the stamp is valid again.

## What the deploy does NOT ship

- **Config.** See [[change-config-on-all-three]].
- **udev rules.** See [[grant-input-access]].

## The deploy is no longer the only path

Since [`015`](../../specs/015-ota.md) the machines also update
themselves from the Releases, and `deploy.sh` ships `oxdesk-ota` and the
token along. The deploy is still the **in-house** path — and the only
one that swaps the binary on Windows, because the Authenticode key
doesn't leave the Ubuntu machine. See [[enable-ota]].

## CI binaries don't replace the deploy

CI builds the three systems on every code push, but **doesn't sign or
install**. An `unsigned` package doesn't go straight to Windows or the
Mac. See [`specs/013`](../../specs/013-ci-cd.md).

## Docs commits don't need a deploy

The build id is the git HEAD, so a documentation-only commit makes the
machines report "behind" without anything functional having changed.
It's not a problem; the next deploy syncs.

## Restarting the Electron shell (tray)

The tray menu lives in Electron's **main process**. `watchBuild()`
reloads the *page* when the build changes, but not the main process — so
a change in `gui/main.js` only takes effect when the app restarts.

```bash
pkill -f 'oxdesk/gui/node_modules/electron/dist/electron'
cd ~/dev/oxdesk/gui && (setsid nohup env -u ELECTRON_RUN_AS_NODE \
  node ./node_modules/.bin/electron . > ~/.local/state/oxdesk-gui.log 2>&1 &)
```

**The pattern used to be `electron \.$` and it failed silently.** With
any argument (`electron . --setup`) the command line doesn't end in `.`
— and the binary itself reorders the arguments to `electron --setup .`.
`pkill` didn't match, the old instance stayed alive, the new one hit
`requestSingleInstanceLock()` and exited, and the log came back empty
because `>` had just truncated it. Symptom: "it didn't start", with the
app running the whole time. The binary path always matches.

**`env -u ELECTRON_RUN_AS_NODE` is not optional.** With that variable in
the environment, electron runs as plain node, `app` comes back
`undefined` and it dies in `app.requestSingleInstanceLock()` — a message
that says nothing about the variable. This has already taken down a
running GUI.

Always check that it came back — killing it without relaunching leaves
the desktop without a tray:

```bash
# `pgrep -af electron` matches the very command you typed
# (the string "electron" is in it) and always "finds" something. Filtering
# by `comm` looks at the executable, not the command line.
ps -eo pid,comm --no-headers | awk '$2 ~ /electron/' | head -3
tail -3 ~/.local/state/oxdesk-gui.log
```

The Vulkan/ozone warning in the log is cosmetic.

### On the Mac and on Windows

The deploy **ships** `main.js` to both (`Resources/app/main.js` on the
Mac, `%LOCALAPPDATA%\oxdesk\gui\main.js` on Windows), but doesn't close
any open window: it takes effect on the app's next launch.

To force it on the Mac:

```bash
ssh mac-host 'pkill -f "Oxdesk.app/Contents/MacOS/Oxdesk"; sleep 1; open -a ~/Applications/Oxdesk.app'
# check WHICH shell loaded -- the renderer process's `--app-path` is
# reliable, and it's the only way to know whether it was `app/` or a
# forgotten `app.asar`:
ssh mac-host 'ps -eo command | grep -o "app-path=[^ ]*Oxdesk[^ ]*" | sort -u'
```

Previously the Mac shell lived in an `app.asar` that **no step touched**
— it was weeks old before anyone noticed. It became a loose `Resources/app/`
(the classic Electron layout, still supported), with `app.asar` renamed
to `.off`, `ElectronAsarIntegrity` removed from `Info.plist` and the
bundle re-signed ad-hoc. The deploy step is idempotent and redoes the
conversion if someone reinstalls the app.

## Open straight into configuration

```bash
node ./node_modules/.bin/electron . --setup
```

Opens fullscreen on the setup page. It's the same as the tray's "Set up
this machine" — see [[configure-a-machine]].
