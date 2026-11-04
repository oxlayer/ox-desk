# As três máquinas

| | Ubuntu | Windows | macOS |
|---|---|---|---|
| nome | `ubuntu-host` | `windows-host` | `mac-host` |
| endereço | local | `windows-host.local` | `mac-host.local` |
| SO | Ubuntu 26.04, GNOME 50.1, Wayland | Windows 10 Pro | macOS 14 (Apple Silicon) |
| usuário | `user` | `user` | `user` |
| ssh | — | `ssh windows-host` | `ssh mac` |

**Nunca escreva IP em config.** O IP do Windows já mudou uma vez e
quebrou tudo de uma vez. mDNS (`avahi` + `libnss-mdns`).

## Caminhos que divergem

É aqui que se perde tempo. Nada disso segue um padrão só.

| O quê | Ubuntu | Windows | macOS |
|---|---|---|---|
| config | `~/.config/logi/config.toml` | `%APPDATA%\logi\config.toml` | `~/.config/logi/config.toml` |
| binários | `~/.local/bin/` | `%LOCALAPPDATA%\logid\` | `~/.local/bin/` |
| processo do agente | `logid` | `logid.exe` | `logid` |
| reiniciar | `systemctl --user restart logid.service` | `taskkill /F /IM logid.exe` + `schtasks /run /tn logid` | `launchctl kickstart -k gui/$(id -u)/ai.logi.logid` |
| log do host de tela | `~/.local/state/logistream-host.log` | `%LOCALAPPDATA%\logid\logistream-host.log` | idem Ubuntu |

Note que a **tarefa** agendada no Windows chama-se `logid` enquanto o
**executável** é `logid.exe` — e o host de tela é `logistream-host` nos
dois. Conferir sempre com
`schtasks /query /fo table | findstr /i logi`.

## Portas

| Porta | Serviço |
|---|---|
| 8787 | API + painel do `logid` (cada máquina serve o seu) |
| 9777 | host de tela `logistream` (QUIC: vídeo por datagrama, input por stream) |
| 22 | SSH (Windows e Mac) |
| 1716 | KDE Connect — **sai quando o clipboard nativo entrar** |
| 5900 | VNC do Mac — fica enquanto o host do Mac não produzir quadro |

RDP (3389) saiu em 04/11: os quatro caminhos de tela são nossos.

## A sessão interativa do Windows

SSH no Windows cai na **sessão 0**, onde não existe desktop. Nada que
precise de tela ou de `SendInput` funciona por SSH direto — nem
duplicar a tela, nem injetar input, nem apagar o monitor.

O que funciona: tarefa agendada com `/IT`, que roda na sessão
interativa. É assim que `logid` e `logistream-host` sobem.

## Veja também

- [[caminhos-de-tela]]
- [[../.claude/rules/config-por-maquina]]
- [[../.claude/rules/reiniciar-agente]]
