# Remote PowerShell: always `-EncodedCommand` from a file

Inline breaks. Not "sometimes": it breaks every time the command has
quotes, `$`, `` ` `` or an accent. It broke so many times in this repo
that it became a rule.

```bash
# do
cat > "$S/task.ps1" <<'PS1'
$p = Get-Process -Name oxdesk -ErrorAction SilentlyContinue
if ($p) { "pid " + $p.Id } else { "not running" }
PS1
ENC=$(iconv -f UTF-8 -t UTF-16LE < "$S/task.ps1" | base64 -w0)
ssh windows-host "powershell -NoProfile -EncodedCommand $ENC"
```

```bash
# don't
ssh windows-host 'powershell -NoProfile -Command "if ($p) { \"x\" }"'
```

The heredoc must be **quoted** (`<<'PS1'`): without the quotes bash
expands `$p` and runs `` ` `` as command substitution. A `` `r`n ``
inside an expanded heredoc once ran as a command, and the step still
said "ok" because the error went to stderr.

## Three specific traps

1. **`$args` is a PowerShell automatic variable** and can't be a
   parameter name.
2. **Output comes with `\r`** — pass it through `tr -d '\r'` before
   comparing.
3. **The CLIXML header and the SSH warnings pollute stdout.** Filter:
   `grep -v "WARNING\|store now\|may need\|openssh\|CLIXML\|^<Objs"`.

## Trap #4: PowerShell doesn't wait for a GUI-subsystem executable

The most expensive of the four, because it **doesn't fail — it is
intermittent**.

`oxdesk.exe` is `windows_subsystem = "windows"`, without a console, so
the daemon doesn't flash a window when started by the scheduled task.
But the same binary serves as the CLI, and PowerShell **doesn't wait**
for a GUI process to finish. Two chained harms:

1. `$LASTEXITCODE` is never set, so `; exit $LASTEXITCODE` returns 0 and
   SSH reports **success** whatever the result.
2. The SSH session closes while the process is still reading stdin. Who
   wins the race depends on the payload size.

Measured on 2026-11-04, three text sends to `oxdesk clip-set`:

| Remote command | Arrived |
|---|---|
| `& "...\oxdesk.exe" clip-set` | **2 of 3** |
| `& "...\oxdesk.exe" clip-set \| Out-String \| Write-Output` | 3 of 3 |

Consuming the output forces PowerShell to wait for the process to end.
Without it, the text clipboard to Windows had been **intermittent from
the start** and went unnoticed: a line of text usually wins the race, and
it was a 50 KB `tar` — which always loses — that revealed the problem.

Use [`remote::oxdesk_command`](../../oxdesk/src/remote.rs), which already
builds the right line per system. Don't build it by hand.

```rust
// do
.arg(crate::remote::oxdesk_command(&host, "clip-arq-set"))

// don't
.arg(r#"& "$env:LOCALAPPDATA\oxdesk\oxdesk.exe" clip-arq-set"#)
```

### Corollary: binary stdin to a child doesn't survive

In the same console-less binary, passing stdin along with
`Stdio::inherit()` delivers **an empty folder and exit 0**. Reading
stdin *inside* the process works — it's what `clip-set` always did, and
that's why it is also the path used in `clip-arq-set`, which writes a
temporary `.tar` instead of letting `tar` inherit the descriptor.

Verification in the interactive session: `windows/probes/read-clipboard.ps1`
via an `/IT` task (over SSH you read the session 0 clipboard, always
empty).

## See also

- [[verify-what-arrived]] — sent is not arrived
