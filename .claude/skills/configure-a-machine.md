# Configure a machine (setup wizard)

Open `http://127.0.0.1:8787/setup` **on the machine you want to
configure**. From the tray: "Configurar esta máquina" (Configure this
machine). The Electron shell opens this page by itself, fullscreen, when
something essential is missing.

```bash
xdg-open http://127.0.0.1:8787/setup     # Ubuntu
ssh windows-host 'start http://127.0.0.1:8787/setup'   # doesn't work: see below
```

> ⚠️ **You can't configure one machine from another.** The wizard
> measures the graphical session of whoever serves it. Over SSH Windows
> lands in session 0 (clipboard always empty) and macOS has no TCC grant
> at all. See [[../rules/graphical-session]].

## Reading the screen

Four states, and the third is the one that matters:

| Mark | Means |
|---|---|
| ✓ green | measured and up |
| ! red | measured and missing |
| ? amber | **I couldn't measure** — it is not "ok" |
| – dimmed | doesn't apply to this machine |

The step highlighted with the blue ring is where to act **now**. Dimmed
steps with "espera por: X" (waits for: X) depend on another one that is
still missing — there's no point touching them before.

The bar counts only the **essential** ones. Pending optional ones show up
in the right corner ("2 opcional(is) pendente(s)") and don't block
"pronto" (ready).

## From the command line

```bash
curl -s http://127.0.0.1:8787/api/setup | python3 -m json.tool
curl -s -X POST http://127.0.0.1:8787/api/setup/arrumar \
     -H 'Content-Type: application/json' -d '{"id":"daemon"}'
```

`/api/setup` takes a few seconds: the `pares` step opens SSH to each
machine. That's why the page does **not** refresh by itself.

## When the button says it didn't fix it

It's telling you what it **measured after** acting, not that the command
failed. Two normal cases:

- **macOS TCC**: the repair opens the system panel and comes back with
  "marque a caixa na janela que abriu" (check the box in the window that
  opened). That's the expected flow — check it and click "Medir de novo"
  (Measure again).
- **`sudo`**: the wizard never runs sudo. Copy the line, paste it in a
  terminal, come back and measure.

## Order that matters on the Mac

`Assinar` (Sign) comes **before** asking for the permissions, and the
wizard enforces it through a dependency. Granting Screen Recording to an
ad-hoc binary wastes the grant: it dies at the next `deploy.sh`, because
TCC ties it to the cdhash, which changes on every build. It has happened
twice.

## Tailscale on the Mac

The `tailscale` step is optional and measures through the brew
`tailscale`. With the app installed it shows as missing, even when
connected: delete the app from Finder and **deny** any VPN request. See
[[../../specs/012-outside-the-lan]].

```bash
brew install tailscale
sudo bash oxdesk/setup/install-tailscale-mac.sh   # in the repo clone
```

The wizard button does the same without a clone: it opens Terminal with
the script.

## Windows without a monitor

The `tela-virtual` step counts the monitors present. With zero, the
viewer connects and no frame arrives. The button installs the driver
from `oxdesk/setup/install-virtual-display.ps1`, with pinned version and
hash. Over SSH, as administrator, the script runs directly, without a
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

It prints `ERROS []` when it passes. The test compares the screen with
what the API measured — counter, number of rows, highlight of the active
step, no row without detail, no horizontal scroll at 420 px. It has
already caught an accordion that kept two steps open while the state
said one.

## See also

- [[../../specs/011-onboarding]] — the design and the three rules
- [[deploy]] — what takes binaries to the three machines
- [[verify-the-mac]] — when the Mac screen doesn't open
- [[grant-input-access]] — the udev rule in detail
