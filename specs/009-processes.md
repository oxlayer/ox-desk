# 009 — Task manager

The user's request: *"see statistics, usage and be able to end a task on
the other PC too"*.

`GET /api/processes?host=<name>` and `POST /api/processes/kill`, plus the
**Tarefas** (Tasks) panel in the dashboard. Works for all three hosts,
local or over SSH.

## One command per OS, chosen by the *target's* OS

There is no command all three understand. The choice comes from
`HostCfg::os` — not from the OS where the daemon runs. Running `ps`
against Windows, or `Get-Process` against Ubuntu, fails **silently**: the
panel would be empty without saying why.

| target | command |
|---|---|
| Linux / macOS | `ps -eo pid=,pcpu=,rss=,comm= \| sort -k2 -rn \| head -n 60` |
| Windows | `Get-Process \| Sort CPU -Desc \| Select -First 60 … \| ConvertTo-Json` |

Sorting is done in `sort` and not with a `ps` flag because the flags
diverge: Linux has `--sort=-pcpu`, macOS has `-r`.

## `cpu` means different things, and that is declared

On Unix, `ps` gives an **instantaneous percentage**. On Windows,
`Get-Process` gives **accumulated seconds** of CPU since the process
started. They are different units and different readings.

Converting would require two samples and is not worth the cost. So the
JSON carries `cpu_kind` (`pct` or `seconds`) and the UI switches the
column header between `cpu %` and `cpu s`. Showing `24715` in a `%`
column would be a plausible lie — the worst kind, because nobody
suspects it.

## Ending a process asks for confirmation, in two steps

Same stance as `/api/focus` and `set-host`: killing a process on someone's
machine does not come from a distracted click, and has no undo. The body
requires `confirm: true`, and the UI arms before acting — the first click
swaps the button for `encerrar | forçado` (end | forced), the second acts.

`pid` 0 and 1 are refused on the server.

On Windows, `taskkill` gets `/T` (the tree): closing only the parent
would leave an orphan child running, and whoever clicked expected it to
go away.

## `remote.rs`: the third time became a function

`probe::ssh_detail` and `agent` were already building `ssh` with the same
four options plus the resolution workaround ([`002`](002-ssh.md)). This
module would have been the third copy — and the fourth would be where
someone forgot `ssh_addr_opts` and broke only on Windows, which is
exactly how that bug showed up the first time.

`remote::run(host, cmd, timeout)` resolves local versus remote, chooses
`sh -c` or `powershell -Command`, and filters the post-quantum warning
that Windows OpenSSH spits on stderr.

## Verified

On all three, through the panel:

- `ubuntu-host` — `local`, `cpu %`, 60 processes
- `windows-host` — `ssh`, `cpu s`, 60 processes
- `mac-host` — `ssh`, `cpu %`, 60 processes

And the two-step confirmation follows the right pid even when the list
reorders between one click and the next.

## Missing

- Machine totals (aggregate CPU, memory) in the panel header.
  `/api/status` already carries free/total RAM; it still needs stitching.
- Filter by name.
