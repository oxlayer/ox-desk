# Logitech Flow → functional equivalent on Ubuntu

Target: **Ubuntu 26.04 / GNOME 50.1 / Wayland** at `192.168.1.44`
↔ **Windows** at `192.168.1.43` (MAC `00:00:5e:00:53:01`).

## The verdict

Real Flow is impossible: it requires **Options+ on both ends**, and
Options+ doesn't exist for Linux. No Linux alternative (Solaar, Piper,
logiops, OpenLogi) implements Flow — Flow is a proprietary protocol
between the two Options+ instances, not a mouse feature. OpenLogi even
syncs Easy-Switch, but it doesn't move the pointer between screens or
share the clipboard.

What gives the **same practical result** is two pieces:

| Flow piece | Substitute | State here |
|---|---|---|
| Pointer crosses the screen edge + keyboard follows | **Deskflow** 1.27.0 (formerly Synergy CE, upstream of Barrier/Input Leap) | installable, `.deb` already downloaded |
| Copy/paste and drag files between the machines | **GSConnect** (GNOME) + **KDE Connect** (Windows) | via apt |
| Battery, Easy-Switch channel, DPI | **Solaar** | via apt |

(`wl-clipboard` went into the first `install.sh` by my mistake and was
removed: it would serve a clipboard workaround of Deskflow itself on
Wayland, which a) asks for the `-git` version, not the apt one, and b) is
unnecessary because the clipboard here is GSConnect's.)

Barrier (2.4.0, 2021) and Input Leap (archived in Jul/2026) are dead —
Deskflow is the same code, maintained, and the only one with Wayland
support via libei/portals.

## Topology: Windows is the server

The **server** is the machine with the physical keyboard/mouse; the rest
are clients. Here the server is **Windows**:

- It's where the Unifying receiver you use is, and where Options+ keeps
  giving you Smart Actions / gestures / buttons.
- On Wayland the **client** role is the documented and stable path
  (`RemoteDesktop` portal, requires GNOME 50+ — you have 50.1).
- The client **dials out** on port 24800, so Ubuntu doesn't need an
  inbound rule.

> A detail I found probing this machine: **there is a second Unifying
> receiver here** (`046d:c52b`) with the **MX Anywhere 3** and the
> **MX Keys** paired. In other words, today you already switch manually
> with the Easy-Switch button. With Deskflow you leave both on the
> Windows channel and stop pressing buttons.
>
> The reverse (Ubuntu as server) is also viable on this machine — I
> verified that `xdg-desktop-portal-gnome` 50.0 declares **and accepts**
> `org.freedesktop.portal.InputCapture` (v1), which the Deskflow
> documentation still lists as "GNOME doesn't implement". It's outdated
> information on their side, but it's an untested path: start with what
> works and invert later if you want.

## Ubuntu (here)

```bash
cd /home/user/dev/logi
sudo bash install.sh
```

Then **log out and back in** once (GSConnect is a shell extension) and
enable it in the **Extensões** (Extensions) app.

## Windows side (192.168.1.43)

Copy `windows-setup.ps1` from this directory to the Windows machine and
run it in PowerShell **as administrator**:

```powershell
powershell -ExecutionPolicy Bypass -File windows-setup.ps1
```

It downloads and installs Deskflow **1.27.0** (the same version as here
— the protocol matches by version), creates the firewall rule on port
24800 and shows whether the network is marked as *Public* (if it is, the
rule doesn't apply — the script prints the command to switch to
*Private*).

Then, by hand:

1. Open Deskflow → **Server** mode.
2. *Configure Server* → drag a new screen into the grid, to the left or
   right of Windows depending on the physical position of the Ubuntu
   monitor. Name the screen **exactly** `ubuntu-host` — it's this
   machine's hostname, and the name is case-sensitive.
3. Install **KDE Connect for Windows** (Microsoft Store) for
   clipboard/files.

## Closing the circuit

On Ubuntu, open Deskflow → **Client** mode → server `192.168.1.43` →
*Start*. On the first connection both ends show a **TLS fingerprint** —
check that it matches and accept.

GNOME will ask for **remote control** permission (`RemoteDesktop`
portal): accept and tick remember, otherwise the dialog reappears every
session.

Then pair GSConnect with KDE Connect on Windows (the GNOME tray lists
the device) and turn on *Clipboard sync* on both ends.

## Order matters (otherwise you end up without a keyboard)

Today the MX Anywhere 3 and the MX Keys are on the channel of the
**receiver here**. When you press Easy-Switch to the Windows channel,
this machine is left **without keyboard and mouse** until Deskflow
connects. So:

1. Windows: run `windows-setup.ps1`, open Deskflow in **Server**, build
   the grid with the `ubuntu-host` screen.
2. Ubuntu: open Deskflow in **Client**, point it to `192.168.1.43`,
   press *Start* — it keeps trying to connect, that's expected.
3. **Only then** press Easy-Switch on both devices to the Windows
   channel.

If something goes wrong in step 3 and you are left without input: press
Easy-Switch back to the channel here, or plug in a USB keyboard. It's
worth keeping one nearby the first time.

## Keyboard layout: `us`-only bites

Symptom: the client connects, the cursor crosses, but accents, `ç` and
the ABNT2 keys come out wrong. In the core log:

```
INFO: local layouts: en
INFO: remote layouts: pt
WARNING: server layouts missing on this computer: pt
```

Windows is on a Portuguese layout and Ubuntu came with only `us`
(`localectl` showed `X11 Layout: us`, and `input-sources` was empty).
Deskflow's `languageSync=true` tries to mirror the server's layout and
has nowhere to go.

### The detail that breaks the obvious fix

Adding `br` to the source list is **not enough**:

```bash
# this does NOT fix it on its own
gsettings set org.gnome.desktop.input-sources sources "[('xkb','us'),('xkb','br')]"
```

Measured: with `[us, br]` configured and the service restarted **after**
that, the core still logged `local layouts: en` and the same warning,
and `setxkbmap -query` stayed on `us`.

The reason is that **GNOME doesn't use xkb groups** — it swaps the whole
keymap according to the active source. So Deskflow only sees **one**
layout: the active one. `br` has to be *active*, not just in the list.

The two ways out, depending on the hardware:

| The keyboard is… | Fix |
|---|---|
| **ABNT2** (its own `ç` key to the right of `L`) | make `br` primary here: `sources "[('xkb','br'),('xkb','us')]"` |
| **US / international** | Windows is the wrong one: change **it** to the US layout |

Choosing the wrong option just moves the problem to the other side.

### In this setup: the keyboard is US, so the fix is on Windows

Ubuntu is right (`us` active) and `input-sources` went back to `[]`, as
it was — no layout selector in the bar. What's left is to align Windows:

*Configurações → Hora e Idioma → Idioma e região →* on the language,
`…` → *Opções de idioma → Teclados →* (Settings → Time & Language →
Language & region → Language options → Keyboards) add **United States**
and remove Portuguese (or toggle with `Win+Space`).

If you write in Portuguese on Windows, don't use plain US — you lose the
accents. Use **United States-International**: same key positions,
accents via dead keys, and it's an English layout, so Deskflow should
report `en` and match Ubuntu. Confirm afterwards with `bash status.sh`:
the Teclado (Keyboard) section stops warning and the log no longer
repeats `server layouts missing`.

After changing it, **restart the core** (`systemctl --user restart
deskflow-client`) — it only enumerates layouts at startup.

## Real state of the two machines (read via SSH)

| | Ubuntu | Windows |
|---|---|---|
| hostname / screen | `ubuntu-host` | `windows-host` |
| IP | 192.168.1.44 | 192.168.1.43 |
| OS | Ubuntu 26.04, GNOME 50.1, Wayland | Windows 10 **Pro** |
| user | `user` | `user` |
| role | client (`coreMode=1`) | server (`coreMode=2`) |
| autostart | `deskflow-client.service` (systemd user) | `Deskflow.lnk` in `shell:startup` |

Grid links (`C:\ProgramData\Deskflow\deskflow-server.conf` — in
**ProgramData**, not APPDATA, because `deskflow-daemon` runs as a
service):

```
section: links
	windows-host:
		right = ubuntu-host
	ubuntu-host:
		left = windows-host
end
```

Windows **right** edge → Ubuntu. Ubuntu **left** edge → Windows.

> In 1.27 the screens live in the general config (`screens\N\name=`) and
> `deskflow-server.conf` holds **only the links**. On Linux it's in
> `~/.config/Deskflow/`; on Windows, in `C:\ProgramData\Deskflow\`.

### The keyboard, with the correct diagnosis

Windows is on the **pt-BR language** but with the **United
States-International keyboard layout** (`0416:00020409`, decoded from
the registry). Deskflow reports the **language**, not the layout — hence
`remote layouts: pt`. The warning is about language sync, not about the
physical mapping.

The difference that may show up in practice: US-International has
**dead keys** (`'` + `a` → `á`); Ubuntu's plain `us` doesn't. But
Deskflow transmits **keysyms**, not scancodes — so it's possible the
server already resolves the composition and sends a ready `á`.

**Test before changing anything**: take the cursor to the Ubuntu screen,
open an editor and type `'a`, `~a`, `ç`. If it comes out right, there's
nothing to fix. If it comes out as a literal `'a`:

```bash
# mirrors the Windows behavior (dead keys)
gsettings set org.gnome.desktop.input-sources sources "[('xkb','us+intl')]"
systemctl --user restart deskflow-client
```

For programmers, `us+altgr-intl` is more comfortable (literal quotes,
accents on AltGr) — but then the two machines diverge again.

## One-pass diagnosis

```bash
bash status.sh
```

Read-only. Checks session, package, config, service, socket, server
reachability, portals, Wayland token, TLS/fingerprint, **active**
keyboard layout, known limits (libportal, Scroll Lock, tray) and
warnings from the last hour. It's the first command to run when
something stops working months from now.

## Autostart and reconnection

### Ubuntu: systemd user service

The package only ships a `.desktop`, no unit — so the repo ships
[`deskflow-client.service`](deskflow-client.service), installed by
`install.sh` into `~/.config/systemd/user/` and enabled:

```bash
systemctl --user status deskflow-client
systemctl --user restart deskflow-client
journalctl --user -u deskflow-client -f
```

It runs the **core**, not the GUI — the tray icon doesn't work on GNOME
Wayland anyway. It's `WantedBy=graphical-session.target` because the
client injects input through the portal/libei and needs the graphical
session.

> ⚠️ Don't leave the GUI running together with the service. Deskflow has
> a single-instance guard, so whichever starts second fails. Use the GUI
> only to edit settings, and close it; the service owns the process.

### Reconnection: it's already automatic, don't write a poller

Measured in the log with the server off:

```
connecting to '192.168.1.43': 192.168.1.43:24800
failed to connect to server: Timed out     ← 2s timeout
retry in 1 seconds
```

A **~3s** cycle, indefinitely, with `dynamicConnectionInterval=true`
adapting. A 60s "is it online?" checker would be **worse**: the TCP
attempt already *is* the detection, and pinging first only adds latency.
The unit's `Restart=always` covers a process crash, not a server
offline — they're different things.

### No permission dialog on every boot

`~/.local/state/Deskflow.state` holds:

```
[client]
xdpRestoreToken=<redacted>
```

It's the `RemoteDesktop` portal token. GNOME restores the grant, so the
remote control permission is asked **once**, not on every login.
If that file is deleted, the dialog comes back.

### Windows: shortcut in the Startup folder

`windows-setup.ps1` creates `Deskflow.lnk` in `shell:startup`. It's
logon, not a service: the server needs a user session to capture
keyboard and mouse, so "run whether user is logged on or not" doesn't
work.

## Ubuntu as server: tested and it does NOT work (1.27.0)

Attempt recorded so it isn't repeated. `deskflow-core server`
**segfaults** on this machine:

```
INFO: initial settings file: /home/user/.config/Deskflow/Deskflow.conf
Segmentation fault (core dumped)      ← exit 139
```

Reproduced twice: with an isolated config in the scratchpad and with
the real config (screens filled in `screens\N\name` +
`deskflow-server.conf` with only `links`, which is the 1.27 format — the
screens left the server config and moved to the general config).

The important point: **it crashes before reaching the portal.** So it's
not the GNOME `InputCapture` gap the Deskflow docs describe — I ruled
that out by testing (`gnome.portal` declares
`org.freedesktop.impl.portal.InputCapture` and `CreateSession` v1 is
accepted). It's a bug in this build's server mode.

Practical consequence: Windows stays the server, and the MX devices
need to stay on its channel. If one day you want to invert it, the
candidates are **lan-mouse** (Rust, Wayland-native, uses the
InputCapture portal that exists here — not in apt) or a future
Deskflow version.

## Managing Windows from here

Inverting the KVM wasn't the way to do this — Deskflow only decides
which keyboard drives. To operate the Windows machine from here:

### SSH (real shell, scriptable)

```bash
sudo powershell -ExecutionPolicy Bypass -File windows-ssh.ps1   # on Windows, as admin
ssh -i ~/.ssh/id_ed25519_windows <seu-usuario>@192.168.1.43 'hostname'
```

[`windows-ssh.ps1`](windows-ssh.ps1) installs OpenSSH Server, sets sshd
to automatic, opens port 22 on the Private profile, sets PowerShell as
the default shell and authorizes the key `~/.ssh/id_ed25519_windows`.

### The key has a passphrase

Set yours (the command is interactive, nobody but you types it):

```bash
ssh-keygen -p -f ~/.ssh/id_ed25519_windows
```

The **public key doesn't change** when adding a passphrase — verified
with `ssh-keygen -y`. So `windows-ssh.ps1` stays valid, and you can set
the passphrase before or after running it, in any order.

So the passphrase doesn't become a nuisance, `~/.ssh/config` already
has:

```
Host windows-host
    HostName 192.168.1.43
    IdentityFile ~/.ssh/id_ed25519_windows
    IdentitiesOnly yes
    AddKeysToAgent yes
```

With `AddKeysToAgent yes` and GNOME's `gcr-ssh-agent`
(`SSH_AUTH_SOCK=/run/user/1000/gcr/ssh`), the passphrase is asked **once
per session** and stays in the agent. After that `ssh windows-host` goes
straight in — and any automation (including an agent running here) uses
the key through the agent, without ever seeing the passphrase.

> ⚠️ The gotcha the script solves: for accounts in the
> **Administrators** group, the Windows sshd **doesn't read**
> `~/.ssh/authorized_keys` — it reads
> `C:\ProgramData\ssh\administrators_authorized_keys`, and **silently
> ignores the file** if the ACL isn't restricted. Hence the
> `icacls /inheritance:r` with permission only for Administrators and
> SYSTEM.

### Old files

Nothing to install: `gvfs-backends` is already here, so Nautilus opens
`smb://192.168.1.43` directly under *Outros locais* (Other Locations).
To mount at a fixed path, `cifs-utils` + fstab.

### The Windows desktop in a window

`freerdp3-x11` or `remmina` (both in apt). Requires Windows **Pro** —
Home has no RDP server. And RDP takes over the local session, so it
doesn't go with simultaneous use or with gaming; for gaming, the way is
to go there physically (Deskflow already solves input) or
Sunshine/Moonlight.

## Clipboard: why Ctrl+V didn't paste

**Deskflow's clipboard doesn't work on Wayland** here (`libportal`
0.9.1 < 0.10). What does that job is the **GSConnect ↔ KDE Connect**
pair, and it was half done:

- GSConnect was installed but **not enabled** (state `INITIALIZED`,
  nothing on 1716). Solved with
  `gnome-extensions enable gsconnect@andyholmes.github.io` — it took
  effect **without re-login**, now `ACTIVE` and listening on
  1716/tcp+udp.
- KDE Connect **didn't exist on Windows**. Installed via
  `winget install KDE.KDEConnect` (26.08.2, downloaded from
  download.kde.org with a verified hash), in
  `%LOCALAPPDATA%\Programs\KDE Connect\bin`, with an autostart shortcut
  for `kdeconnect-indicator.exe` created in `shell:startup`.

Once it was open on Windows, the third cause was still missing: **the
KDE Connect installer doesn't create a firewall rule**. `kdeconnectd`
(PID 5584) was listening on 1716 over TCP and UDP, and
`Get-NetFirewallRule -DisplayName "*KDE*"` returned nothing — it spoke
and nobody heard. Same cause as Deskflow at the start. Solved from here,
over SSH:

```powershell
New-NetFirewallRule -DisplayName "KDE Connect (TCP)" -Direction Inbound `
  -Action Allow -Protocol TCP -LocalPort 1716-1764 -Profile Private,Domain
New-NetFirewallRule -DisplayName "KDE Connect (UDP)" -Direction Inbound `
  -Action Allow -Protocol UDP -LocalPort 1716-1764 -Profile Private,Domain
```

Reachability verified **in both directions**: 1716 open from here to
there, and `Test-NetConnection 192.168.1.44 -Port 1716` → `True` from
there to here (`install.sh` had already allowed it in ufw).

### Paired, and one-way on purpose

State verified over DBus:

```
Name: 'windows-host'   Connected: true   Paired: true
EncryptionInfo: 'Verification key: 4BCF0AF8'
last-connection: 'lan://192.168.1.43:1716'
```

The clipboard plugin has two keys, **both `false` by default** — that's
why Ctrl+V didn't paste even when paired:

| key | value here | effect |
|---|---|---|
| `receive-content` | **`true`** | the Windows clipboard arrives here |
| `send-content` | `false` | the clipboard **from here doesn't leave** |

Asymmetric on purpose: Bitwarden runs on this machine, so leaving
sending off contains the
[password manager risk](#automatic-clipboard--password-manager)
without losing the real use case ("I copied there, pasted here"). To
mirror in both directions, turn on `send-content` aware of this.

> ⚠️ **`gsettings set` didn't persist** for this key — it reported
> `true` on `get` but `dconf read` came back empty and `clipboard/`
> didn't even appear in `dconf list`. I don't have a clean explanation.
> What worked:
>
> ```bash
> ID=04d5cb9ad5c9460c943b5465be66bc86
> dconf write /org/gnome/shell/extensions/gsconnect/device/$ID/plugin/clipboard/receive-content true
> ```
>
> Always check with `dconf read`, not with `gsettings get`.

### Testing the clipboard over SSH doesn't work

`Set-Clipboard` over SSH runs in another session: `Get-Clipboard` in the
same session returns the value, but the interactive session's
`kdeconnectd` never sees it. The same isolation that prevents launching
a GUI over SSH. The test has to be manual, copying on the machine
itself.

> `kdeconnect-cli --list-devices` over SSH is **useless** as a
> diagnostic: it runs outside the user's dbus session and returns
> `0 dispositivos` with a Qt window handle error, even with everything
> working.

## Screen sharing: it already exists, no need to build it

### Ubuntu seen from Windows — zero extra software

`gnome-remote-desktop` 50.2 **is already installed and running** here,
with RDP **enabled** on 3389, its own TLS certificate and credentials
configured. Verified: listening, and Windows reaches it
(`Test-NetConnection … -Port 3389` → `True`).

From Windows: `mstsc` → `192.168.1.44`. Microsoft's own client.

```bash
grdctl status          # see state, port, TLS fingerprint
grdctl rdp disable     # turn off, if you don't want this exposed on the LAN
```

> ⚠️ This is an RDP server with password authentication open to the
> whole LAN, on the main machine. Know that it's on — and turn it off if
> it isn't a conscious use.

### Windows seen from Ubuntu

Windows is **10 Pro**, so it has an RDP server — it's just off (3389
closed). Turning it on there, `remmina` or `freerdp3-x11` from here
solve it. Caveat: RDP takes over the local session, it's not for
simultaneous use or for gaming.

### For gaming / low latency

**Sunshine** (host on Windows) + **Moonlight** (client here). Hardware
encoding, latency an order of magnitude lower than RDP or RustDesk.

### Why not build a RustDesk

It would be: capture (PipeWire portal on Wayland, DXGI on Windows),
H.264/AV1 encoding, transport with latency tuning, NAT traversal, input
injection (libei), and the crypto/auth layer. That's the scope of a
product, not of a weekend project — and RustDesk is open source and
**self-hostable** (`hbbs`/`hbbr`), which already meets "own and secure"
without writing anything. On a LAN you don't even need a relay: direct
connection.

## `logid` — the control plane (Rust)

In [`logid/`](logid/). A 929 KB binary, no runtime.

```bash
cargo build --release          # inside logid/
install -m755 target/release/logid ~/.local/bin/logid
systemctl --user enable --now logid.service
```

Config in `~/.config/logi/config.toml`, written on first boot if it
doesn't exist. **An invalid config doesn't silently become the
default**: the daemon refuses and exits with code 1.

| Route | What |
|---|---|
| `GET /healthz` | alive? **0.19 ms** — doesn't touch the network |
| `GET /api/status` | machine state (~2 s; SSH dominates) |
| `GET /api/config` | effective config |
| `PATCH /api/config` | change; validates, writes, and only then applies |
| `GET /api/devices` | HID++ inventory (receiver, slots, online/offline) |
| `POST /api/focus` | hand over input focus — **dry-run by default** |
| `GET /` | the panel |

`/healthz` exists because of a real mistake: the Electron health check
hit `/api/status` with an 800 ms timeout, and that endpoint takes 2 s
because it opens SSH to fetch uptime and RAM. It always failed on a cold
cache.

### PATCH really validates

```
{"campo_que_nao_existe":1}   -> 400  campo desconhecido: ...
{"refresh_seconds":"texto"}  -> 422  invalid type: string
{"port":0}                   -> 422  port invalida
{"hosts":[]}                 -> 422  precisa de ao menos um host
nao-e-json                   -> 400  JSON invalido
[1,2,3]                      -> 400  esperado um objeto JSON
```

It writes to disk **before** applying in memory: if the disk refuses,
nothing changes. And the page starts serving the new `refresh_seconds`
without restarting the daemon.

### `POST /api/focus` — the piece that exists nowhere else

Hands input focus to another machine, choosing between the **physical**
route (HID++ 0x1814 tells the device to jump Easy-Switch channel) and
the **LAN** route (Deskflow). See
[`specs/005`](specs/005-input-switch.md).

**Dry-run by default.** Without `"confirm": true` it only returns the
plan, because the physical route is **irreversible from the source
side**: after `ChangeHost` this receiver no longer reaches the device.

```bash
curl -X POST -d '{"host":"windows-host"}' localhost:8787/api/focus
```

Today the plan returns three real blockers, and that's what it should
do:

```
[ok   ] host                     windows-host (windows-host.local)
[ok   ] alcance                  windows-host.local responde
[ok   ] hidraw                   ACL de uaccess presente
[BLOQ ] posse do dispositivo     'MX Anywhere 3' está offline aqui
[BLOQ ] posse do dispositivo     'MX Keys Keyboard' está offline aqui
[BLOQ ] dead-man's switch        o agente do Windows ainda não existe
```

With `"confirm": true` and pending blockers, it answers **409** and
doesn't execute. The LAN route is honest about its own limit: Deskflow
switches screens by cursor movement or hotkey **on the server**, not by
an external command — there's no way to force focus over the LAN from
here.

And the plan brings `recovery` along, so the GUI can show it **before**
the switch: Easy-Switch button on the device, USB keyboard, SSH from
another machine.

### HID++ inventory

`GET /api/devices` reads via Solaar and reports what matters:
```json
{ "hidraw_access": true,
  "receivers": [{ "name": "Unifying Receiver", "usb_id": "046d:C52B",
    "devices": [ {"slot": 1, "name": "MX Anywhere 3", "online": false},
                 {"slot": 2, "name": "MX Keys Keyboard", "online": false} ]}]}
```

`online: false` means the device is on **another** channel — and while
it is, no HID++ command reaches it. That's the constraint that defines
the architecture.

> The backend is Solaar (parsing `solaar show`). A known and temporary
> fragility: the module boundary was designed so that HID++ 0x1814
> directly over `/dev/hidraw` replaces it without touching the callers.

#### The `uaccess` gotcha

If `hidraw_access` comes back `false`, the likely cause is timing: the
Solaar rule grants access through `TAG+="uaccess"`, and it **only
applies ACLs to nodes created after it**. Here the `hidraw` nodes were
born at 11:37 and Solaar came in at 13:22 — result, `solaar show` died
with `No supported device found`.

```bash
sudo udevadm trigger --subsystem-match=hidraw --action=add
getfacl /dev/hidraw0 | grep user     # confirm
```

### The Windows agent

[`agent/`](agent/) — Rust, cross-compiled from here, **352 KB**, no
runtime:

```bash
cd agent
cargo +stable build --release --target x86_64-pc-windows-gnu
scp target/x86_64-pc-windows-gnu/release/logid-agent.exe \
    windows-host:'C:/Users/user/AppData/Local/logid/logid-agent.exe'
```

Toolchain: `rustup target add x86_64-pc-windows-gnu` + apt
`gcc-mingw-w64-x86-64`. The crate's `.cargo/config.toml` points to the
linker.

It's a **CLI, not a server**, invoked over SSH: zero new ports, zero new
auth. `logid` talks to it through the `ssh windows-host` that already exists.

Read from the hardware via HID++ 0x1814/0x1815:

```
slot 1 (MX Anywhere 3):     3 canais, atual = 2
slot 2 (MX Keys Keyboard):  3 canais, atual = 2
```

That is, **`windows-host` is channel 2**, confirmed — no longer a guess.
The `ubuntu-host` channel is still unknown and **out of the config on
purpose**: sending the device to the wrong channel leaves it unreachable
from both machines. To find out, press Easy-Switch until it shows up
here and run `solaar config <slot> change-host`.

> ⚠️ On Windows the receiver exposes **one HID collection per report
> type** (usage 1 = short `0x10`, 2 = long `0x11`, 4 = very long).
> Writing the wrong size to the wrong collection makes the write fail
> with an empty `hidapi error:`. The agent always uses the long report
> on usage 2, to write and read on the same collection. On Linux a
> single `hidraw` carries all types — the problem doesn't exist there.

### Battery and buttons

```
GET  /api/controls           controls of each ONLINE device
POST /api/controls/remap     {slot, cid, to, confirm}
```

The battery comes via HID++ in two features, because devices of the
same family use different ones: **0x1004** (percentage) on the MX
Anywhere 3 and **0x1000** (range) on the MX Keys. Validated against
`upower`, which reads through the kernel driver and reports the same.

A missing reading uses the **last known one**, with `battery_age_s` —
because HID++ can't read the battery of something that doesn't answer,
and "5% 2 min ago" is worth more than "?" in a tray. Pretending it's a
current reading would be worse.

Remapping refuses a control that the device itself says isn't
reprogrammable: left and right report `flags=0x01`, without the `0x10`
bit. And `to: "0"` restores the default by remapping the control **to
itself** — with zero, the write returns `ok` and nothing changes.

> ⚠️ A low battery **breaks** long enumeration. The keyboard has 24
> controls and at 5% it can't sustain the ~50 round trips; the mouse,
> with 7, completes. The error comes out per device, without bringing
> down the response.

### Three states per service

Two gave false alarms — the SSH that doesn't exist here on purpose and
the Windows RDP turned off by choice:

| `expect` | Meaning |
|---|---|
| `up` (default) | must be up; red if it goes down |
| `down` | must be closed (24800 **here**: this machine is a client) |
| `any` | informational, never alarms |

Adding a third machine is a `[[hosts]]` block in the TOML.

### GUI (Electron)

In [`gui/`](gui/):

```bash
cd gui && npm install && ./run.sh
```

Native window, tray, single instance, closing hides. **It doesn't
duplicate logic**: `logid` serves the UI and the GUI wraps it. If
`logid.service` is up it reuses it; if the port is free, it starts a
child — and only then kills it on exit.

Electron pinned at **44.9.0**. Don't guess the version: I assumed 38.x
and the current line is 45.x. Check with `npm view electron time --json`
and prefer one that is ≥ 2 weeks old.

> ⚠️ The VS Code extension host exports `ELECTRON_RUN_AS_NODE=1`, and
> with it the Electron binary runs as plain Node and **nothing opens** —
> that's why `electron --version` returned the Node version. `run.sh`
> and `npm start` clear the variable.

## The IP changes — and that broke everything once

On 2026-11-03 Windows moved from `192.168.1.43` to `192.168.1.42`
(same MAC `00:00:5e:00:53:01`, confirmed with `ip neigh`). A new DHCP
lease. The damage:

| | Before | Result |
|---|---|---|
| Deskflow | `remoteHost=192.168.1.43` | **KVM dead**, log looping `No route to host` |
| SSH | `HostName 192.168.1.43` | `No route to host` |
| panel | `addr = "192.168.1.43"` | host marked offline |
| GSConnect | discovery by broadcast | **survived** — reconnected by itself on `.102` |

The lesson is in the last row: what discovers by name/broadcast healed
itself; everything I had pinned to an IP broke.

### Fix applied: name instead of IP

Windows publishes its own name over mDNS and this Ubuntu resolves it —
`getent hosts windows-host.local` → `192.168.1.42`, stable. The
mechanism:

```
/etc/nsswitch.conf:  hosts: files mdns4_minimal [NOTFOUND=return] dns
avahi-daemon:        active
libnss-mdns:         0.15.1-5
```

So all configs switched to **`windows-host.local`**: `Deskflow.conf`,
`~/.ssh/config` and `~/.config/logi/config.toml`. Deskflow reconnected
(`ESTAB → 192.168.1.42:24800`) with no IP written anywhere.

> ⚠️ **Deskflow's comma-separated list doesn't work**, even though the
> GUI tooltip says "May contain a comma separated list".
> `remoteHost=windows-host.local,192.168.1.42` made the core abort with
> `FATAL: No server addresses configured`. Use **one** host only.

### The known_hosts trap

Swapping the IP for a name made SSH fail with `Host key verification
failed`: the key was in `known_hosts` under the IP, not under the name.
The fix is **not** a blind `accept-new` — it's copying the *same*
already trusted key:

```bash
KEY=$(ssh-keygen -F 192.168.1.42 -f ~/.ssh/known_hosts | grep -v '^#' | head -1 | awk '{print $2, $3}')
echo "windows-host.local $KEY" >> ~/.ssh/known_hosts
```

Fingerprints checked afterwards, identical:
`SHA256:w3vRoMELMTEZz1EdlfqgvXeW1e9tpXS2OzDntbtwB3Y`.

Note that `ssh-keyscan windows-host.local` **fails** with `Temporary
failure in name resolution` even with `getent` working — a quirk of
`mdns4_minimal` with `[NOTFOUND=return]` on an AF_UNSPEC query. `ssh`
itself resolves normally.

### What's still left to do (on the router)

mDNS depends on avahi and multicast working. For redundancy, create a
**DHCP reservation** on the router for MAC `00:00:5e:00:53:01`. Then the
IP stops dancing and the name keeps working — belt and suspenders.

## Limits you will feel

- **Deskflow's own clipboard doesn't work on Wayland** here: it needs
  `libportal` ≥ 0.10 and Ubuntu 26.04 ships 0.9.1. That's exactly why
  GSConnect comes in — it does clipboard and files on its own, without
  depending on that.
- **Drag-and-drop of files** between screens doesn't exist in Deskflow
  on Linux (nor in Barrier/Input Leap). File transfer goes through
  GSConnect/KDE Connect, which is "share to device" instead of dragging.
- The **cursor stays visible** on the screen you left (cosmetic,
  Wayland).
- **Deskflow's tray icon** doesn't work on GNOME Wayland — keep the
  window open, or put `deskflow-core client` in autostart (1.27 has only
  two binaries: `deskflow`, the GUI, and `deskflow-core`, which takes
  `server` or `client` as an argument).
- **Scroll Lock on freezes** screen switching (it's the "lock to current
  screen" shortcut, inherited from Synergy).
- `AltGr` going from a Linux server → Windows client is problematic;
  since the server here is Windows, it doesn't affect you.

## Security and telemetry (audited in the installed binaries)

| Package | Goes out to the internet? | Note |
|---|---|---|
| `deskflow` 1.27.0 | **no**, except an opt-in update check | GPL-2. Zero telemetry/analytics/Sentry/crashpad/activation. The only URLs in the binaries are SVG icon metadata, the license text and the contributors link on GitHub. |
| `solaar` | **no** | No network socket. The `import socket` lines are only `gethostname()`, to write the machine name to the device (the label the MX Keys shows per channel). |
| `gsconnect` | **no** | No third-party host in the code — only references to RFCs/docs in comments. Direct transport between the two machines on LAN port 1716. |

Deskflow's update check (`cbAutoUpdate`, `stable`/`beta` channels,
`X-Deskflow-Version` header) is the **only** thing in the set capable of
talking to the internet. The URL isn't compiled into the binary — it
comes from the config — and the package doesn't ship a default
`.conf`/schema. It's inert until you tick *Check for updates on startup*
in the GUI.

### What actually crosses the network

It doesn't leave your LAN, but it **does leave this machine**:

- **Clipboard** (GSConnect): goes straight to Windows over
  `192.168.1.x`, in TLS with keys exchanged at pairing, with no
  intermediate server and no cloud. No third party sees it — but it's a
  real copy traveling on the local network.
- **Keys and movement** (Deskflow): everything you type on the remote
  screen travels as input events, in TLS.

That's why **checking the fingerprint on the first connection is for
real**, and not just clicking accept: it's what prevents someone on the
same network from impersonating the server and receiving what you type.

### Automatic clipboard × password manager

Automatic sync **mirrors what you copy from Bitwarden**. A password
copied here becomes a password in the Windows clipboard, and stays
there until overwritten.

Prefer GSConnect's **manual** mode (push/pull on demand) over continuous
mirroring. In the device preferences, under *Clipboard*, turn off *Sync
automatically* and keep only the manual actions — you lose transparency,
you gain not leaking a password to the other machine by accident.

## If it doesn't connect

```bash
# is Windows listening?
timeout 2 bash -c 'echo > /dev/tcp/192.168.1.43/24800' && echo aberto || echo fechado
```

When I probed just now, **all** ports on `.108` showed up filtered
(including 445/3389) and ICMP didn't answer, even though ARP resolves —
that is, the machine is alive and the **Windows firewall is blocking
everything**. Count on step 2 above being mandatory, not optional.

To see the handshake, run the core directly in the terminal instead of
the GUI:

```bash
deskflow-core client --new-instance
```

The log level and the server address come from
`~/.config/Deskflow/Deskflow.conf` (the GUI writes that file), so
configure once in the GUI and then use the command line to diagnose.

## Extra: remapping buttons on Linux

`solaar` already covers battery, scroll and channel (`solaar config`
lists the devices; `change-host` switches the Easy-Switch channel from
the command line — you can bind it to a GNOME shortcut). For
DPI/SmartShift and finer remapping there's **OpenLogi** (`.deb` on their
site).

⚠️ One app per receiver: OpenLogi and Solaar fight over HID++ access.
Pick one.
