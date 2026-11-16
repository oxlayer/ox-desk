# 026 — End-to-end tests in virtual machines

> Written for: whoever changes how OxDesk moves keyboard and mouse between
> machines, and whoever runs a release.

Until now every switching change was proven by hand on the reference
deployment, with a person moving the mouse while someone read the logs.
It worked -- and it means a regression is found by the person who uses
OxDesk, after the release. This spec builds the same proof in two virtual
machines that a script drives, so it runs before a release, without anyone
at a keyboard.

The idea was found in another open-source KVM
([`EduardoMG12/mickey`](https://github.com/EduardoMG12/mickey),
`tools/distro-test/`), which boots throwaway VMs and drives them with
QEMU's own input. The design below is ours.

## What it proves

The chain a person uses, end to end:

1. a **real** key press and mouse move arrive at the source machine -- from
   QEMU's virtual keyboard and mouse, which Windows sees as hardware, not
   as `SendInput` (our hook passes injected input through, so a test with
   `SendInput` would not exercise the grab at all);
2. the source grabs, by the shortcut and by the edge;
3. the target applies it: typed text lands in a file, the cursor moves;
4. the way back, by the edge and by the shortcut;
5. the safety nets: a key held across the edge comes up at home; a
   connection killed mid-press leaves nothing held on the target.

Pass is an effect read on the target (the file's text, a log line with a
count), never "the command exited 0".

## The machines

One VM on each machine of the reference deployment, both under QEMU so one
set of tools drives them:

- **The target on the Ubuntu machine** (VT-x, 12 cores, 60 GB of RAM): QEMU
  with KVM, no root needed, user-mode networking for the internet and SSH.
- **The source on the Windows machine**, on its data disk: the Ubuntu disk
  had 36 GB free, and a Windows VM needs ~25 GB at its peak. QEMU there uses
  the Windows Hypervisor Platform, which a person enables once (admin and a
  reboot).
- **Between them, the LAN:** the target's input and SSH ports are forwarded
  on the Ubuntu machine, on ports of their own, so they never collide with
  the real OxDesk running there.

| VM | Role | Image |
|---|---|---|
| `target-ubuntu` | receives | Ubuntu 26.04 cloud image + `ubuntu-desktop-minimal`, GNOME on Wayland, automatic login |
| `source-windows` | sends | Windows 11, unattended install, OxDesk from the release under test |

`tools/e2e-vm/target-ubuntu.sh` builds and runs the first: cloud-init reads
its seed from a tiny HTTP server on the host through the SMBIOS serial, so
neither a seed ISO nor `cloud-image-utils` is needed. State lives in
`~/.cache/oxdesk-e2e`, never in the repository.

### The Windows VM, decided (2026-11-16)

- **The image:** the public Windows 11 ISO from Microsoft, unactivated. The
  Enterprise evaluation needs a registration form; the maintainer chose the
  unactivated public image instead.
- **TPM:** the unattended install bypasses the check (`LabConfig`), so no
  `swtpm` and no root are needed.

## How input gets in

QEMU's QMP `input-send-event`: key and pointer events delivered through
the VM's virtual USB keyboard and tablet, so to the guest they are
hardware. The driver types a known string, moves to an edge, pushes back,
and holds a key across the edge.

## What it reads

- **On the target:** a text file the typed string lands in (an editor
  opened on it, saved with Ctrl+S, read over SSH); the host's own log
  (`input: announced the screen size`, `released N key(s)/button(s)`).
- **On the source:** the KVM's end-of-session lines (`forwarded: ...`,
  `release(s) ... went to this machine`), the daemon's `kvm: input going to
  ... in N ms`.
- **Pictures:** QMP `screendump` of both screens at each step, kept with the
  run as evidence; not judged by the script.

## When it runs

On every release candidate, not on every pull request: it is slow, and the
PR gate stays fast. A failure blocks the tag.

## What the VMs cannot prove

- **NVENC and the screen stream:** no GPU in the VMs. The input path does not
  need one; video stays proven on the reference deployment.
- **The Logitech receiver and HID++:** no USB passthrough. The LAN KVM does
  not depend on it.
- **Feel:** acceleration and latency as a person perceives them. The
  numbers in the logs still are measured.

## Order of work

1. The Ubuntu target VM, built and booting to a GNOME session: **done**
   (2026-11-16; QEMU screenshot shows the desktop logged in).
2. OxDesk installed in it from the release, the host answering on its
   input port, proven with `oxdesk-input-send` from the host.
3. The Windows source VM, after the image question is answered.
4. The driver (QMP input) and the first full run: type, cross, come back.
5. The safety nets as scripted cases.

## See also

- [`025`](025-native-switching.md) — what these tests protect
- [`024`](024-windows-kvm-source.md) — the Windows source
