# Restarting the agent: the process name is not the binary name

Each OS has its own mechanism, and the names do **not** follow a
pattern. Getting the name wrong fails silently: `Stop-Process` on a
process that doesn't exist is not an error, and `schtasks /run` on a
task that is already running does nothing. The result is "I restarted"
without having restarted.

| Machine | Process | Command |
|---|---|---|
| Ubuntu | `logid` | `systemctl --user restart logid.service` |
| macOS | `logid` | `launchctl kickstart -k gui/$(id -u)/ai.logi.logid` |
| Windows | **`logid.exe`** | `taskkill /F /IM logid.exe` + `schtasks /run /tn logid` |

The scheduled task on Windows is called **`logid`**, and the executable
**`logid.exe`** — not `logid-agent`, even though the deploy talks about
`logid-agent.exe` in other contexts. Check with
`schtasks /query /fo table | findstr /i logi`.

## `taskkill`, not `Stop-Process`

Over SSH, `Stop-Process -Force` **does not kill a process in the
interactive session**. A `logistream-host` survived three attempts and
held port 9777, making the following tests fail for the wrong reason — I
lost time thinking capture was broken.

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
