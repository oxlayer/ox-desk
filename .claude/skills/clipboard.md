# Clipboard between the machines

Text synced across the three, without KDE Connect. Design details in
[[../../specs/004-clipboard.md]].

## How to check that it is alive

```bash
# Ubuntu -> peers
echo -n "teste-$(date +%s)" | xclip -selection clipboard
sleep 5
ssh mac 'pbpaste'
```

On Windows you **can't** check over SSH: the clipboard is per window
station, and `Get-Clipboard` from session 0 reads a different one. It
needs an interactive task:

```powershell
schtasks /create /tn ler-clip /tr "powershell -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$ps`"" /sc once /st 00:00 /it /f
schtasks /run /tn ler-clip
```

where the script does `Get-Clipboard -Raw | Out-File ...` and you read
the file. The same session boundary that shows up with the cursor and
the screen host ([[../rules/cursor-without-mouse]]).

## When it doesn't propagate

The source `logid` log tells you. `journalctl --user -u logid -n 30 | grep clip`:

| Message | Cause |
|---|---|
| `command not found: logid` | wrong remote path — the SSH shell doesn't load the login PATH |
| `recusou (exit status: 1)` with empty stderr | the peer's `logid` is old and doesn't know `clip-set` — deploy |
| `nao consegui falar com <host>` | SSH doesn't connect: key, mDNS or machine offline |
| nothing in the log | the source didn't see the copy; on Windows/Mac the sampling is every 2 s |

## The Mac key

The Mac reaches the Ubuntu machine through the `ubuntu` alias in its
`~/.ssh/config`, which carries the right `IdentityFile`. Testing with
`user@ubuntu-host.local` gives `Permission denied` even with
everything right — it's the alias that has the key. Always use the alias
that the `logid` config declares in `ssh = `.

If the key isn't authorized on this side:

```bash
ssh mac 'cat ~/.ssh/id_ed25519_logi.pub' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
```

## Limits

**Text** only, capped at 256 KB. Images and files don't travel — KDE
Connect did that; if it's missed, it's new work.

---

# File clipboard

Same path as text (SSH, no new port), but the nature is different: text
**is** the content; a file is a reference, and the destination clipboard
must point to files that exist on its disk. That's why the bytes travel
on **Ctrl+C**, not on Ctrl+V — there's no way to intercept the paste on
all three systems.

## The three limits

| Limit | Value | What it decides |
|---|---|---|
| `TETO_EMBUTIDO` | 8 MiB | goes by itself to all peers |
| `TETO_TOTAL` | 2 GiB | above this it's an accident, refuse |
| `TETO_ARQUIVOS` | 2000 | same, for a large but light tree |

Between 8 MiB and 2 GiB we do **not** send by ourselves: the tray shows
"Mandar arquivos para `<host>`" (Send files to `<host>`) and you pick a
destination. Copying 1.4 GB to two machines nobody is looking at wastes
someone else's bandwidth and disk.

Above 8 MiB to a unix destination it goes by `rsync --partial`; with
Windows at either end it goes by `tar`, because Windows has `tar.exe`
and `scp.exe` in `system32` but **has no rsync** (measured).

## Test without a GUI

Copy a file on Ubuntu without clicking in Nautilus:

```bash
printf 'file://%s\r\n' "$(python3 -c "import urllib.parse,sys;print(urllib.parse.quote(sys.argv[1]))" "/caminho/com espaço.txt")" \
  | xclip -selection clipboard -t text/uri-list
```

On Windows and macOS the copy must be born in the **graphical session**:

```bash
# Windows: /IT task (over SSH it's session 0, with its own empty clipboard)
scp windows/sondas/por-arquivo-no-clipboard.ps1 'windows-host:AppData/Local/logid/'
ssh windows-host 'schtasks /create /tn por-arquivo /tr "powershell -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File $env:LOCALAPPDATA\logid\por-arquivo-no-clipboard.ps1" /sc once /st 00:00 /it /f; schtasks /run /tn por-arquivo'

# macOS: the pasteboard is per user, so over SSH it works
ssh mac 'osascript -l JavaScript -e "ObjC.import(\"AppKit\");const pb=\$.NSPasteboard.generalPasteboard;pb.clearContents;pb.writeObjects(\$([\$.NSURL.fileURLWithPath(\"/tmp/x.txt\")]))"'
```

## Check what arrived

```bash
# Ubuntu: the three types must appear
xclip -selection clipboard -t TARGETS -o
xclip -selection clipboard -t x-special/gnome-copied-files -o   # starts with `copy`

# Windows: interactive session, via /IT task
ssh windows-host 'schtasks /run /tn ler-clipboard; Start-Sleep 3; Get-Content $env:LOCALAPPDATA\logid\clipboard.txt'

# macOS
ssh mac 'osascript -l JavaScript /tmp/ler-pb.js'
```

`x-special/gnome-copied-files` always starts with **`copy`**, never
`cut`: `cut` tells the destination to delete the source after pasting,
and the source is on another machine. We have no way of knowing the
paste happened, and deleting someone's file by mistake has no undo.

## Traps that have already cost time

- **PowerShell doesn't wait for a GUI process.** The text clipboard to
  Windows had been intermittent forever because of this. See
  [[../rules/remote-powershell]], trap #4.
- **A repeated name is refused**, not renamed: two `igual.txt` from
  different folders would land on top of each other in the staging
  folder and you would paste one thinking there were two.
- **The staging folder is `~/.cache/logi/clipboard/`**, outside `/tmp`:
  on some distros `/tmp` is tmpfs, and 2 GB of clipboard in RAM is a way
  to meet the OOM killer. It keeps the last 5 deliveries.
