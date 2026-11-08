# One supervisor per daemon — and never the GUI

It cost a whole day. The Mac served a ten-day-old binary while **every**
deploy step said `ok`, and the one who noticed was the user.

## What was happening

The GUI (`gui/main.js`) started the daemon on its own when `/healthz`
didn't answer. That seems polite — "if nobody answers, I'll start it" —
but it creates a **second owner of port 8787**:

```
boot → app.oxdesk.gui starts first → pings, nobody answers → spawn(oxdesk)
     → app.oxdesk.daemon (LaunchAgent) starts later → Address already in use
     → exit 1 → launchd throttles → state = spawn scheduled, forever
```

The winner of the race is an **orphan**: `launchctl print` doesn't know
it, `launchctl kickstart -k` doesn't kill it, `systemctl restart` doesn't
kill it. The loser is precisely the copy that the deploy and the OTA
know how to restart.

Result: `install -m755` wrote the new file, `kickstart -k` restarted a
job that died in a loop, the orphan kept answering with the old build —
and every step returned exit 0.

## The three rules

### 1. The OS is the supervisor

`systemd --user`, `launchd`, `schtasks`. The GUI **asks** (`systemctl
--user start`, `launchctl kickstart`, `schtasks /run`) and only starts a
standalone child if there is no supervisor installed — the case of a new
machine, where the GUI is how you reach the wizard.

### 2. Whoever restarts clears the port first

`pkill -x oxdesk` before `kickstart -k` / `systemctl restart`. It's not
brutality: `-k` already kills the supervised copy anyway, so the net
effect is exactly *"also the ones the supervisor doesn't know about"*.
It is in `plan::Plan::clear` (OTA) and in the `deploy.sh` steps.

OTA without this is worse than the deploy: it restarts, asks `/healthz`,
hears the orphan's old build and **rolls back an update that was
correct**.

### 3. The panel compares who serves with what is on disk

`oxdesk versao` prints the stamp of the **file**; `env!("OXDESK_BUILD")` is
the stamp of **who answers**. A divergence becomes a panel line
(`setup_common::stamp_verdict`). Being on disk is not being live.

And "job loaded" is not "job running": `launchctl print` brings `state`
and `last exit code`, and ignoring them approved a crash loop
(`setup_common::launchd_verdict`). The panel said "loaded in the
graphical session" about a daemon that hadn't come up since boot.

## See also

- [[verify-what-arrived]] — telling it to restart is not having restarted
- [[restart-the-agent]] — the same lesson on the Windows side
- [[graphical-session]] — why `launchctl` needs `gui/<uid>/`
