# 004 — Clipboard

## What already works

GSConnect ↔ KDE Connect, paired and with TLS. Verified:
`Paired: true`, `Connected: true`,
`EncryptionInfo: 'Verification key: 4BCF0AF8'`.

The **Deskflow clipboard does not work on Wayland** here: it needs
`libportal` ≥ 0.10 and Ubuntu 26.04 ships 0.9.1. That is why the
transport is KDE Connect.

## Policy is the product, protocol is not

The protocol is solved. What is missing is **policy**, and that is where
our system adds value:

| GSConnect key | Value here | Reason |
|---|---|---|
| `receive-content` | `true` | the Windows clipboard arrives here |
| `send-content` | `false` | the clipboard **from here does not leave** |

Asymmetric on purpose: Bitwarden runs on this machine. Automatic sync in
both directions would mirror a copied password to the other machine,
where it stays until overwritten.

### What we want beyond that

- **Blocklist by source app.** If focus was on Bitwarden, do not
  propagate — not even with sync on. This is the policy KDE Connect
  lacks.
- **TTL.** A received clipboard expires after N seconds.
- **Explicit manual push** always available, independent of auto.
- **Optional local history**, never synchronized.

## Recorded traps

**`gsettings set` does not persist** on these keys: it reported `true`
on `get` while `dconf read` came back empty and `clipboard/` did not
appear in `dconf list`. No clean explanation. Use `dconf write` and check
with `dconf read`.

**Testing over SSH does not work.** `Set-Clipboard` over SSH writes to a
clipboard that the interactive session's `kdeconnectd` does not see —
and `Get-Clipboard` in the same SSH session returns the value, which
makes the test deceptively green. Clipboard tests have to run **in the
agent**, inside the graphical session.

**Firewall.** The KDE Connect installer **does not create a rule** on
Windows. It was listening on 1716 TCP/UDP and `Get-NetFirewallRule
-DisplayName "*KDE*"` returned nothing. Rules for 1716-1764 TCP+UDP are
part of the bootstrap ([`002`](002-ssh.md)).

## Implemented (2026-11-04) — and KDE Connect is out

Text clipboard synchronized across the three machines, with no third
parties.

### Linux: Mutter's private API

Neither `wl-clipboard` (Wayland does not let you read the selection
without focus; `ext-data-control` does not appear among this
compositor's globals) nor the `org.freedesktop.portal.Clipboard` portal
(it exists, but is tied to a session with a permission dialog).

The same `org.gnome.Mutter.RemoteDesktop` we already use to inject input
provides the whole clipboard — `EnableClipboard`, `SelectionRead`,
`SetSelection`, `SelectionWrite`, plus the `SelectionOwnerChanged` and
`SelectionTransfer` signals — and the session **comes up without
ScreenCast**, which was probed before writing anything.

Two gotchas, both found by probing and both commented in the code:

1. **`EnableClipboard` with `mime-types` makes us the selection owner.**
   Reading then fails with *"Tried to read own selection"*. To observe,
   call it without types; declare types only when publishing.
2. **The `SelectionRead` fd comes non-blocking.** `read_to_end` on it
   returns `WouldBlock` immediately — and whoever swallows that error
   reads zero bytes and concludes the clipboard is empty. That is
   exactly what the first probe concluded, with the clipboard full.

### Transport: SSH, not a new port

The API listens only on `127.0.0.1` by design (specs/008). The clipboard
carries passwords fairly often; opening a LAN port for it would be new
surface for the most sensitive data we now move.

SSH is already the trusted channel between the machines. ~300 ms per
copy, which is invisible in real use: you copy, switch machines, and
only then paste.

The remote binary's path comes **per system**, not from PATH: SSH's
non-interactive shell does not load the login PATH, and the Mac replied
`zsh:1: command not found: oxdesk`. The config's `agent` field does not
work either — it points to another executable and only exists on Windows.

### Why `clip-set` does not write directly

On Linux, whoever writes the selection has to **stay alive** to answer
`SelectionTransfer` when someone pastes. A CLI that writes and exits
would leave an ownerless selection, and the text would vanish on the
first Ctrl+V. So the CLI hands off to the local daemon (POST on
127.0.0.1) and the daemon stays.

### Echo loop

A would send to B, B would see the clipboard change and send it back,
forever. `registrar()` keeps the last text seen **or written**, in both
directions; a change that matches it is not resent.

### Windows and macOS: polling

Both have a notification API (`AddClipboardFormatListener`,
`NSPasteboard`'s `changeCount`) that would require a message loop or
objc inside a daemon that has neither. Polling every 2 s does the job:
you cannot feel it when switching machines.

A detail that would cost time: PowerShell's `Get-Clipboard` appends a
CRLF at the end. Without trimming, every read would look like new text
and the machine would push the same content forever.

### Checked

All four directions, with a timestamp on each:

| From → To | Result |
|---|---|
| Ubuntu → Mac | `ubuntu-171000` |
| Ubuntu → Windows | `ubuntu-171000` |
| Mac → Ubuntu | `mac-171127` |
| Windows → Ubuntu | `windows-171146` |

The Windows clipboard has to be read **inside the interactive session**:
each session has its own window station, and a `Get-Clipboard` over SSH
reads session 0's, which is a different one. Same boundary that already
showed up with the cursor and the screen host.

### Known limits

- **Text only.** Images and files do not travel. KDE Connect did that;
  if it is missed, it is new work.
- 256 KB cap. A developer's clipboard sometimes carries an entire file
  pasted by accident.
- The Mac needed its key authorized on Ubuntu (`Host ubuntu` in its
  `~/.ssh/config` already existed; this side accepting it was missing).
