# Each machine has its own config, and nothing syncs

Three machines, three `config.toml` files, **zero synchronization**.
`deploy.sh` ships binaries; it doesn't ship config. Changing one and
thinking you changed them all is the standard mistake.

| Machine | Path |
|---|---|
| Ubuntu | `~/.config/oxdesk/config.toml` |
| macOS | `~/.config/oxdesk/config.toml` (via `ssh mac`) |
| Windows | `%APPDATA%\oxdesk\config.toml` (`C:/Users/user/AppData/Roaming/oxdesk/`) |

`oxdesk/default-config.toml` is only the seed for a new installation.
Editing it does **not** change any machine that is already running.

## How this came up

We removed RDP from the panel by editing the Ubuntu config and
`default-config.toml`. The Ubuntu panel cleared. The user replied:
*"kdp disappeared from ubuntu but I still see it in the windows and mac
gui"* — because each agent reads its own machine's config and serves its
own panel.

## Procedure

See [[../skills/change-config-on-all-three]]. In short: download,
transform with the SAME script on all three, upload, restart each one's
agent. Never edit the three by hand — divergence gets in through typing.

## See also

- [[restart-the-agent]] — process and task names differ per OS
- [[../skills/change-config-on-all-three]] — the recipe
