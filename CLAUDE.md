# logi — entry point for agents

Personal infrastructure for **three** computers on the same LAN: Ubuntu
(main), Windows next to it, and a MacBook Air. It started as "a Logitech
Flow that works on Linux" and grew: switching machines, our own remote
screen, and control of everything from a single panel.

| Directory | Contents | When to read it |
|---|---|---|
| [`.claude/rules/`](.claude/rules/) | Inviolable rules — traps that have already cost time | Before touching any machine |
| [`.claude/skills/`](.claude/skills/) | Procedures — recipes that worked | When the task is "how do I do X again" |
| [`docs/`](docs/) | Domain knowledge with [[wikilinks]] | To understand how the pieces fit together |
| [`specs/`](specs/) | Specifications, from `000-overview` to `017-product` | To understand *why* the system is the way it is |
| [`README.md`](README.md) | Historical runbook — what I tried and what failed | Before reintroducing something that was removed |

## Rules (executive summary)

1. [Each machine has its own config, and nothing syncs](.claude/rules/config-per-machine.md)
2. [Restarting the agent: the process name is not the binary name](.claude/rules/restart-the-agent.md)
3. [Remote PowerShell: always `-EncodedCommand` from a file](.claude/rules/remote-powershell.md)
4. [`E_ACCESSDENIED` in DXGI: four causes, and the most common is the screensaver](.claude/rules/windows-locked-screen.md)
5. [Windows without a mouse has no cursor, and drawing it in the stream doesn't help whoever looks at the monitor](.claude/rules/cursor-without-mouse.md)
6. [What depends on the graphical session is not born over SSH](.claude/rules/graphical-session.md)
7. [Sent is not arrived; a number nobody prints, nobody checks](.claude/rules/verify-what-arrived.md)
8. [Capture pixel ≠ injection point](.claude/rules/coordinate-spaces.md)
9. [Code for another OS compiles here — switch the `cfg` and check](.claude/rules/check-the-other-cfg.md)
10. [One supervisor per daemon — and never the GUI](.claude/rules/one-supervisor-per-daemon.md)

## Skills (procedures)

- [Deploy to the three machines](.claude/skills/deploy.md)
- [Change config on the three machines](.claude/skills/change-config-on-all-three.md)
- [Grant access to the input devices (KVM)](.claude/skills/grant-input-access.md)
- [Test the screen host with the screen off](.claude/skills/test-with-screen-off.md)
- [Clipboard between the machines](.claude/skills/clipboard.md)
- [Verify screen, mouse and keyboard on the Mac](.claude/skills/verify-the-mac.md)
- [Configure a machine (setup wizard)](.claude/skills/configure-a-machine.md)
- [Reach the machines outside the LAN](specs/012-outside-the-lan.md)
- [Enable and operate automatic updates (OTA)](.claude/skills/enable-ota.md)

## Docs

- [[docs/index]] — index
- [[docs/machines]] — the three machines, paths and names that differ
- [[docs/screen-paths]] — who captures, encodes and decodes on each pair

## The components

| Dir | What |
|---|---|
| [`logid/`](logid/) | Control plane in Rust: API, panel, machine switching, probes |
| [`logistream/`](logistream/) | Remote screen and KVM: capture, NVENC/NVDEC, QUIC, input |
| [`hidpp/`](hidpp/) | HID++ 2.0 over raw HID — Easy-Switch channel switching |
| [`gui/`](gui/) | Electron shell: window and tray. **No logic** |
| [`agent/`](agent/) | Windows side: CLI over SSH |
| [`ota/`](ota/) | `logi-ota`: downloads the Release, verifies, signs, swaps and rolls back |

## Decisions not to be undone without justification

**Deskflow and Solaar are gone.** Machine switching is physical (HID++
0x1814) and edge detection is ours. Solaar competed for the same
receiver.

**Never write an IP in config.** The Windows IP changed once and broke
Deskflow, SSH and the panel all at once. Use mDNS, or the tailnet name on
the machine that leaves the house ([`specs/012`](specs/012-outside-the-lan.md)).

**On the Mac, Tailscale is the brew one, never the app.** Its network
extension took down the machine's internet on 2026-11-07.

**RDP and KDE Connect are gone** (2026-11-04). The four screen paths are
ours, and so is the clipboard — through Mutter's private API on Linux,
with SSH as transport. Text only, for now.

**One dongle only.** Two Logitech receivers close together don't work:
the pairing follows the dongle, not the machine. See [`specs/005`](specs/005-input-switch.md).

## Working rules in this repo

1. **Verify, don't assume.** Half the findings here contradict the
   official documentation — and half contradict what I myself wrote in
   the previous commit. GNOME's `InputCapture` exists and does **not**
   work for enabling capture on demand; the "locked screen" I diagnosed
   was a screen that was off; the "faulty" dongle is not proven. Test
   before asserting, and correct what you already asserted.
2. **Measure before optimizing, and print what you measured.** Two bugs
   survived for weeks for lack of one log line.
3. **Record the failures, not just the successes.** They are worth more
   than what works, because they avoid repeating the lost afternoon.
4. **UI in pt-BR; code, commits and file names in English.** Scripts,
   binaries, modules, environment variables and config keys too
   (`sign.sh`, not `assinar.sh`). The old Portuguese names go away along
   with the switch to oxdesk ([`specs/017`](specs/017-product.md)).
5. **Minimal telemetry that can be turned off; cloud optional; LAN
   always.** Since 2026-11-07 ox-desk is a product
   ([`specs/017`](specs/017-product.md)): trust comes from each
   machine's key, the server is ours or the user's, and with LAN alone
   everything keeps working. The old trust model is in
   [`specs/008-security.md`](specs/008-security.md).
