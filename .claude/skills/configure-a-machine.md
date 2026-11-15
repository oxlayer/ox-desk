# Configure a machine (setup wizard)

Open `http://127.0.0.1:8787/setup` **on the machine you want to
configure**. From the tray: "Set up this machine". The Electron shell
opens this page by itself, fullscreen, when something essential is
missing.

```bash
xdg-open http://127.0.0.1:8787/setup     # Ubuntu
ssh windows-host 'start http://127.0.0.1:8787/setup'   # doesn't work: see below
```

> **You can't configure one machine from another.** The wizard measures
> the graphical session of whoever serves it. Over SSH, Windows lands in
> session 0 (clipboard always empty) and macOS has no TCC grant at all.
> See [[../rules/graphical-session]].

## Reading the screen

Four states, and the third is the one that matters:

| Mark | Means |
|---|---|
| green dot, "done" | measured and up |
| red dot, "to do" | measured and missing |
| amber dot, "error" | **could not measure** — it is not "ok" |
| hollow dot, "n/a" | doesn't apply to this machine |

The step to act on **now** is tinted with the accent colour and opens by
itself. Dimmed
steps marked "waiting for X" depend on another step that is still
missing; there's no point touching them before it.

The bar counts only the **essential** steps. Pending optional ones show
up in the right corner ("2 optional pending") and don't block "ready".

## From the command line

```bash
curl -s http://127.0.0.1:8787/api/setup | python3 -m json.tool
curl -s -X POST http://127.0.0.1:8787/api/setup/fix \
     -H 'Content-Type: application/json' -d '{"id":"daemon"}'
```

`/api/setup` takes a few seconds: the `peers` step opens SSH to each
machine. That's why the page does **not** refresh by itself.

## When the button says it didn't fix it

It's reporting what it **measured after** acting, not that the command
failed. Two normal cases:

- **macOS TCC**: the repair opens the system panel and comes back with
  "tick the box in the window that opened and measure again". That's the
  expected flow: tick it and click "Measure again".
- **`sudo`**: the wizard never runs sudo. Copy the line, paste it in a
  terminal, come back and measure.

## Order that matters on the Mac

Signing (the "Screen host signed" step) comes **before** asking for the
permissions, and the wizard enforces it through a dependency. Granting
Screen Recording to an ad-hoc binary wastes the grant: it dies at the
next `deploy.sh`, because TCC ties it to the cdhash, which changes on
every build. This has happened twice.

## Tailscale on the Mac

The `tailscale` step is optional and measures through the Homebrew
`tailscale`. With the Tailscale app installed it shows as missing, even
when connected: delete the app from Finder and **deny** any VPN request.
See
[[../../specs/012-outside-the-lan]].

```bash
brew install tailscale
sudo bash oxdesk/setup/install-tailscale-mac.sh   # in the repo clone
```

The wizard button does the same without a clone: it opens Terminal with
the script.

## Windows without a monitor

The `virtual-display` step counts the monitors present. With zero, the
viewer connects and no frame arrives. The button installs the driver
from `oxdesk/setup/install-virtual-display.ps1`, with a pinned version
and hash. Over SSH, as administrator, the script runs directly, without a
prompt on screen.

## New computer

The order per system is in [[../../docs/new-computer]].

## What it doesn't do

It doesn't install binaries (use `install.sh` or `scripts/deploy.sh`),
doesn't run sudo, and doesn't keep measuring in a loop.

## Check that the page still works

```bash
cd gui && env -u ELECTRON_RUN_AS_NODE \
  ./node_modules/electron/dist/electron check-setup.js
```

It prints `ERRORS []` when it passes. The test compares the screen with
what the API measured — counter, number of rows, highlight of the active
step, no row without detail, no horizontal scroll at 420 px. It has
already caught an accordion that kept two steps open while the state
said only one was.

## See also

- [[../../specs/011-onboarding]] — the design and the three rules
- [[deploy]] — what takes binaries to the three machines
- [[verify-the-mac]] — when the Mac screen doesn't open
- [[grant-input-access]] — the udev rule in detail
