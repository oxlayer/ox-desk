# 003 — File browser

## Decision: SFTP, not a new protocol

OpenSSH is already on both machines and SFTP comes with it. There is no
reason to invent a transport: what is missing is **UI**, not protocol.

KDE Connect has an `sftp` plugin (it is in `windows-host`'s
`incoming-capabilities`), but it depends on its daemon and mounts via
KIO — outside our control. Plain SSH is simpler and already authenticated.

## Two layers

**Browsing (`/api/files/{id}?path=`)** — lists a directory via `sftp`
in batch mode. Read-only, fast, nothing mounted:

```bash
echo "ls -la /caminho" | sftp -b - windows-host
```

**Mounting (optional)** — `sshfs` (already installed here) for when you
want to open it in Nautilus or an editor:

```bash
sshfs windows-host:/C:/Users/user ~/mnt/windows-host -o reconnect,idmap=user
```

Warning: `sshfs` dies badly when the host goes down, and the host
**already went down once today** because of an IP change. `-o reconnect`
helps; even so, mounting is an optional convenience, never the main path
of the UI.

## Paths on Windows

Windows SFTP exposes `/C:/Users/user/...`. The UI should show
`C:\Users\user` and translate internally — the user should not see the
seam.

## Transfer

Small file: `scp`/`sftp` directly.
A file you want to "send to the other machine" as a gesture: the
**KDE Connect share** already works and lands in `~/Downloads`
(`receive-directory` in dconf). Do not reimplement.

## Do not

- Do not index or cache the directory tree. LAN latency is low; an
  invalid cache is worse than listing again.
- Do not write by default. Writes only on an explicit user action.

---

## Implemented (2026-11-03)

[`oxdesk/src/files.rs`](../oxdesk/src/files.rs) + "Arquivos" (Files) panel
in the dashboard. **Listing only** in this version: no download, no
upload, no delete.

The daemon is a privilege surface ([`008`](008-security.md)) and can
already run SSH, so listing grants nothing new — but writing does, and
writing by accident from a panel I serve myself is the kind of mistake
that has no undo.

### Route

`GET /api/files?host=<name>&path=<path>`

Without `host`, it uses the `local` host from the config — the common
case of opening the panel just to look. Without `path`, each host starts
at its own home (SFTP `pwd` on the remote, `$HOME`/`%USERPROFILE%`
locally). `path` comes percent-encoded because it carries `/`, `\`, `:`
and spaces, and none of them can be mistaken for a query separator.

The route became `?host=` instead of the `/api/files/{id}` this spec
proposed: hosts are already identified by **name** everywhere else in
the API (`/api/focus` takes `host`), and introducing a parallel `id`
would be a second way of saying the same thing.

### One SFTP batch, not two

The first version called `pwd` and then `cd`+`ls -la`, paying for two
SSH handshakes: **809 ms** measured to list a folder. With `pwd` and
`ls` in the same batch: **350 ms**.

As a bonus, `pwd` in the same batch gives the canonical path confirmed by
the server, and `display` and `parent` come from it — the UI navigates by
what the server says, not by what I assumed.

### The parser is anchored on the month, not on a field count

I wrote "name = everything after the 8th field". There are **nine**
(mode, links, owner, group, size, month, day, time, name), and the test
caught it: `.` and `..` slipped through the filter because the name read
was another piece of the line, and the "size" read was the group name.

Fixing the index to 9 would not be enough — some SFTP servers omit the
group, and then there are eight. Looking for the **month** field finds
the junction in both cases, because from there to the right the format
is fixed (`<month> <day> <time-or-year> <name>`) and the size is the
field immediately before it. Locked by
`parses_even_without_the_group_field` and
`year_instead_of_time_for_old_file`.

The name is cut by **byte offset** in the original line, not by `join`ing
the remaining fields: `Program Files` has a space, and joining would
collapse repeated spaces.

### Paths are not comparable between the two sources

SFTP exposes the Windows disk as `/C:/Users/user`; `std::fs` on the
machine itself says `C:\Users\user`. They are two names for the same
place and neither is wrong. Instead of normalizing (and getting it
wrong), each listing declares its `source`, returns an **opaque** `path`
for the UI to navigate, and a separate `display` to show —
`C:\Users\user` on screen, `/C:/...` on click.

### Date: each source says what it actually knows

Local gives a unix epoch (`mtime`), SFTP gives the `ls` text
(`mtime_text`) because the protocol gives no epoch in this listing. The
UI prefers the epoch and formats it in the local time zone; it falls back
to the text when that is what exists. Inventing precision the source
does not have would be worse than showing `Nov 3 22:19`.

### Verified

Cap of 2000 entries per listing (`truncated` warns). 17 unit tests.
End-to-end through the UI itself, in a headless Electron:

- `ubuntu-host` → `/home/user`, 84 items, `local`
- click on `windows-host` → `C:\Users\user`, 105 items, `sftp`
- enter `.agents` → `C:\Users\user\.agents`
- click on "voltar" (back) → back to `C:\Users\user`, 105 items

### Missing

- **Windows → Ubuntu does not list**, and says so on screen instead of
  spinning: `openssh-server` is not installed on Ubuntu. Same blocker as
  the edge symmetry in [`001`](001-control-plane.md).
- Transfer and `sshfs`: designed above, not implemented.
- Mac: comes in together with the new host.
