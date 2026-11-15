# Restarting the agent: the process name is not the binary name

Each OS has its own mechanism, and the names do **not** follow a
pattern. Getting the name wrong fails silently: `Stop-Process` on a
process that doesn't exist is not an error, and `schtasks /run` on a
task that is already running does nothing. The result is "I restarted"
without having restarted.

| Machine | Process | Command |
|---|---|---|
| Ubuntu | `oxdesk` | `systemctl --user restart oxdesk.service` |
| macOS | `oxdesk` | `launchctl kickstart -k gui/$(id -u)/app.oxdesk.daemon` |
| Windows | **`oxdesk.exe`** | `taskkill /F /IM oxdesk.exe` + `schtasks /run /tn oxdesk` |

The scheduled task on Windows is called **`oxdesk`**, and the executable
**`oxdesk.exe`** — not `oxdesk-agent`, even though the deploy talks about
`oxdesk-agent.exe` in other contexts. Check with
`schtasks /query /fo table | findstr /i oxdesk`.

## `taskkill`, not `Stop-Process`

Over SSH, `Stop-Process -Force` **does not kill a process in the
interactive session**. An `oxdesk-host` survived three attempts and
held port 9777, making the following tests fail for the wrong reason —
and time went into the theory that capture was broken.

`taskkill /F /IM name.exe` kills it.

## Verify, always

Restarting without checking is half the job. After the restart, hit the
API:

```
curl -s localhost:8787/api/status
```

If the panel still shows the old state, the agent didn't restart.

## See also

- [[config-per-machine]] — why you are restarting
- [[remote-powershell]] — how to send this without breaking on quotes
