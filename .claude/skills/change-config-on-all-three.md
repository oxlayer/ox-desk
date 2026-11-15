# Change config on the three machines

Config doesn't travel with the deploy. Changing one machine and assuming
all three changed is the standard mistake — see
[[../rules/config-per-machine]].

The rule of the procedure: **one transformation, applied three times**.
Editing the three files by hand introduces divergence through typing.

## 1. Write the transformation as a script

```python
# remove-rdp.py — the transformation used on 2026-11-04
import re, sys, pathlib
p = pathlib.Path(sys.argv[1])
s = p.read_text()
before = s
s = s.replace('units = ["oxdesk.service", "gnome-remote-desktop.service"]',
              'units = ["oxdesk.service"]')
s = re.sub(r'\n *\[\[hosts\.services\]\]\n *label = "RDP[^"]*"\n *port = 3389\n(?: *expect = [^\n]*\n)?', '\n', s)
p.write_text(s)
print("changed" if s != before else "already clean", "->", p)
```

Printing "changed" vs "already clean" is what makes the operation
idempotent and auditable: running it twice breaks nothing, and an
"already clean" on a machine you expected to change reveals that the
pattern didn't match there.

## 2. Ubuntu (local)

```bash
python3 -I remove-rdp.py ~/.config/oxdesk/config.toml
systemctl --user restart oxdesk.service
```

## 3. macOS

```bash
scp mac-host:.config/oxdesk/config.toml /tmp/mac-config.toml
python3 -I remove-rdp.py /tmp/mac-config.toml
scp /tmp/mac-config.toml mac-host:.config/oxdesk/config.toml
ssh mac-host 'launchctl kickstart -k gui/$(id -u)/app.oxdesk.daemon'
```

## 4. Windows

The path is `%APPDATA%\oxdesk\`, which in `scp` becomes
`C:/Users/user/AppData/Roaming/oxdesk/`.

```bash
scp 'windows-host:C:/Users/user/AppData/Roaming/oxdesk/config.toml' /tmp/win-config.toml
python3 -I remove-rdp.py /tmp/win-config.toml
scp /tmp/win-config.toml 'windows-host:C:/Users/user/AppData/Roaming/oxdesk/config.toml'
```

Restart the agent. The process is **`oxdesk.exe`**, the task is
**`oxdesk`**, and the tool is `taskkill`, not `Stop-Process`
([[../rules/restart-the-agent]]):

```powershell
taskkill /F /IM oxdesk.exe
Start-Sleep -Seconds 2
schtasks /run /tn oxdesk
```

## 5. Check all three

Don't trust each step's "ok":

```bash
curl -s localhost:8787/api/status                     # ubuntu
ssh mac-host 'curl -s localhost:8787/api/status'      # mac
# windows: Invoke-WebRequest http://127.0.0.1:8787/api/status
```

If one still shows the old state, its agent didn't restart.

## And `default-config.toml`

Also update `oxdesk/default-config.toml` in the repo — it is the seed for
a new installation. It doesn't change any machine that is already
running, but without it the next installation brings back what you just
removed.
