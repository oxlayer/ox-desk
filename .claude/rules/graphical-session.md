# What depends on the graphical session is not born over SSH

Three systems, three mechanisms, **the same rule**: screen capture and
input injection only work in a process launched from inside the user's
session. SSH is not that on any of them.

| System | Where SSH lands | How to really launch |
|---|---|---|
| Windows | session 0, no desktop | scheduled task with `/IT` |
| macOS | session without a TCC grant | LaunchAgent (`gui/$(id -u)/...`) |
| Linux | works, but the process dies with the connection | `setsid nohup` |

## macOS is the worst of the three, because it doesn't fail

On Windows the error shows up: `E_ACCESSDENIED`, `SendInput` refused. On
macOS capture **hangs**. Without the Screen Recording grant,
AVFoundation simply never returns.

Measured to leave no doubt it wasn't our code:

```
ffmpeg -f avfoundation -framerate 60 -capture_cursor 1 -i 1 -frames:v 5 -f null -
```

hangs forever when called over SSH. The same command in the graphical
session runs. Our host stopped before the first `println!`, with the
process in state `S` and no socket open — a symptom of blocking, not of
an error.

## The grant is manual, and there is no way around it

Screen Recording is granted in **Ajustes → Privacidade e Segurança →
Gravação de Tela** (Settings → Privacy & Security → Screen Recording),
and it can't be done from outside: it is exactly the protection Apple
built. For a LaunchAgent without an interface the system may not even
show the dialog.

Once granted, everything unlocks at once:

```
avfoundation: tela 1, 3360x2100, UYVY422
h264_videotoolbox pronto, 28 Mbps alvo
host na porta 9777
```

468 frames in 10 s, 46.8 fps, zero lost.

**Watch out the next time the binary changes:** TCC ties the grant to
the executable's hash. In theory a rebuild invalidates it and the host
goes back to hanging with the same silence. If that happens after a
deploy, don't look for a bug — it's the grant. (Not yet observed here;
noted as a suspicion, not as a fact.)

## `setsid` doesn't exist on macOS

The line that woke up unix hosts was `setsid nohup ...`. On the Mac it
died with `command not found` **before** reaching nohup — the host never
started and the symptom arrived as "it called VNC". Today `setsid` is
conditional.

## See also

- [[windows-locked-screen]] — the same boundary, on the Windows side
- [[cursor-without-mouse]] — another case of "the symptom is not where you look"
