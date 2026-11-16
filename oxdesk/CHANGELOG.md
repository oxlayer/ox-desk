# What's new

One entry per change you can notice while using it. It is embedded in the
binary (`include_str!`), so what the panel shows is always what that
machine is running, not what the repository has.

Format: `## <date> · <title>` and a description underneath.

## 2026-11-16 · The edge follows your monitors
With monitors of different sizes side by side, part of a screen's edge
did not switch, and coming back could put the cursor where no screen is.
The edge is now worked out per monitor, and the cursor comes back onto a
real one, at the height it left at.

## 2026-11-15 · Web pages can no longer control OxDesk
The local API answered any web page open in a browser on the same
computer, and a page could have sent your keyboard to another machine or
changed settings. It now accepts commands only from its own panel, the
tray and the command line.

## 2026-11-15 · Going there is as quick as coming back
Crossing the edge toward the other machine now takes about 50 ms instead
of 300 to 450: the other side is woken while the mouse is still on its way
to the edge, so reaching it has nothing left to wait for.

## 2026-11-15 · Back by the screen edge, and switching in half a second
With the edge on, pushing the mouse past the other screen's edge that faces
this machine now brings the keyboard and mouse back -- a firm push, so a
scroll bar on that edge does not. The cursor comes in at the same height it
left at. And a switch takes about half a second instead of five: the
daemon no longer asks the other machine, over SSH, things it already knows.

## 2026-11-15 · Windows can send its keyboard and mouse to another machine
With the Logitech receiver plugged into Windows, both Ctrl, the screen
edge and "Send to" in the panel or the tray now move the keyboard and the
mouse to the other machine over the network, the way Linux always did.
Both Ctrl bring them back, even if the network is down. Before this, the
only way out of Windows was the device's own Easy-Switch button.

## 2026-11-15 · Choose the keys of the shortcut
Settings › Switching machines: both Ctrl, both Shift or both Alt. The same
keys send the keyboard and mouse away and bring them back, and a tip on the
Overview says which ones are in use, so the gesture is no longer a secret.

## 2026-11-15 · The edge turns on without a restart, and says what it is doing
Turning the edge on in the panel used to need a daemon restart nobody was
told about. It now starts within a second, and Settings shows its state in
words: watching, waiting because the keyboard and mouse are on another
machine, or why it cannot run here.

## 2026-11-15 · Ubuntu's remote screen follows the monitor you use
The Ubuntu host recorded a fixed monitor connector. When the monitor moved
to another port it recorded an invisible virtual screen instead, and the
mouse sent from another machine moved there. It now asks GNOME which
monitor is the primary one.

## 2026-11-04 · The picture no longer freezes until you move the mouse
The window loop blocked waiting for a system event, and the network thread
filled the slot with the new frame without waking anyone up, so the
picture only updated when you moved the mouse or typed. The report
("it freezes on a chat message until I move the mouse") described the
exact mechanism. Now the network wakes the window on every frame:
measured, 720 frames drawn at 59 fps with nobody touching anything.

## 2026-11-04 · Input from the Mac to Ubuntu: stuck keys and lag
Every event was a call to the compositor **waiting for a reply**, on the
same thread that pumps the video, so the key waited for the frame and the
frame waited for the key. Now injection does not wait for a reply (there
is nothing useful in it). The Ubuntu host also keeps to 60 fps instead of
sending 80: measured, 4.5 Mbps against 6.2, with no visible difference on
a 60 Hz screen. And the Mac viewer copies pixels a row at a time instead
of one by one: that was 921 thousand iterations per frame to produce
bytes that already arrived in the right format.

## 2026-11-04 · RDP is gone: Windows watches through our protocol too
The viewer now builds there, decoding with NVDEC. Windows has no
`libavcodec`, and that alone was what kept that path on mstsc. All four
paths between the three machines are ours now. Measured from here with
the GPU: 840 frames at 65 fps, none lost.

## 2026-11-04 · GPU decoding, with no dependency on ffmpeg
NVDEC decodes our stream: 2560x1440 at 250 fps, 4 ms per frame including
the trip back to the CPU. The colour conversion was checked against
ffmpeg on the same frame (mean R=60 G=60 B=61 against R=60 G=59 B=61, one
step of rounding). This is what will unlock the viewer on Windows, where
there is no libavcodec, and with it the last RDP in the system.

## 2026-11-04 · The panel can ask the device where its channels go
The `hosts <slot>` command reads the hosts stored in the keyboard itself
(0x1815), with the name and radio type of each channel, unifying or
bluetooth. That ended an hour of wrong investigation: I was inferring the
pairing from what the receivers show, and a receiver cannot name a device
that is not there. The inventory now also says `paired`, separating
"empty slot" from "paired, but the device is on another machine". Before,
the two were the same JSON.

## 2026-11-04 · A button to pause reading the devices
The devices go back to channel 1 on their own, with the edge and the
reconciler off on all three and no log line claiming it. With all of our
channel-switching code ruled out, two hypotheses were left: our HID++
reads disturbing the radio, or the hardware. Layout now has a button that
pauses polling for 10 minutes. Switch machines during the pause: if it
stays, the problem is ours; if it goes back on its own, it is the radio.
Switching channels keeps working during the pause.

## 2026-11-04 · Uptime, RAM and CPU on every card, always
The machine you were looking at had no detail at all: the reading went
over SSH and nobody SSHs into themselves, which is why you could see the
MacBook's RAM from Windows and not from the MacBook itself. Now the local
card reads locally. Linux RAM had never worked (the two numbers came out
glued together because they depended on the order of /proc/meminfo), and
the bug survived because this is the only Linux machine and its detail
was never asked for. CPU came along, and the line always shows, with a
dash when it could not be read, instead of disappearing.

## 2026-11-04 · Cursor in the right place, and full screen that stretches
Two defects of mine, from the same earlier fix. The mouse position used
the picture's size as if it were the remote screen's: in a 1280x720
window against 2560x1440, your cursor only reached the top-left quarter.
And drawing required an exact size match, so on entering full screen
every frame was discarded and the old, small buffer stayed in the corner.
Now the picture fits the window without distortion (with bars when the
aspect ratio differs) and the cursor is mapped through the picture, not
through the window.

## 2026-11-04 · Panel with a sidebar, favourites and one pane at a time
In the style of the Windows app that was pointed out: a translucent
sidebar with the machines at the top, favourites right below and the
panes listed, and the content switches in place instead of being one long
scroll of five sections. The star pins a machine to Favourites; the ▶
next to it opens its screen directly. In a narrow window the sidebar
becomes a horizontal bar.

## 2026-11-04 · "View screen" for Windows, from here, is ours now
The viewer now builds on Linux (libavcodec from apt), so the button on
the Ubuntu panel no longer calls RDP. Three defects came out on the way:
reconnecting was refused, a failed handshake took down the whole host,
and the first frame of a reconnection sometimes was not a keyframe, so
the window opened black with the decoder complaining. Now nothing goes
out before a keyframe, by guarantee and not by luck.

## 2026-11-04 · The Windows edge obeys the switch too
I had fixed only the GNOME one. Windows has its own implementation, and
the log proved the damage: two seconds after it was turned off on screen,
it armed itself and handed mouse and keyboard back to Ubuntu. Now both
read the flag on every pass, and the dead man's switch, which handed the
devices back if you did not type for 25 s, stops along with them.

## 2026-11-04 · Windows serves its own screen through our protocol
DXGI capture, GPU encoding, QUIC: the same path as Ubuntu. Checked from
here: 2560x1440, 174 frames in 10 s, none lost. On the Mac panel,
"View screen" for Windows is ours now, not RDP. On the Ubuntu panel it
is still RDP, because the viewer only builds on macOS for now, and
swapping a button that works for one that says "no client" would not be
progress.

## 2026-11-04 · The panel shows each machine's edge and reconciler
The edge is a behaviour of a pair: the Windows one, pointed this way,
handed mouse and keyboard back at every touch, while this screen said,
correctly, that the edge here was off. It was invisible without opening
the other machine's panel. Now each card shows the state of that host's
two switches, in amber when they are on.

## 2026-11-04 · The edge and reconciler switches now turn things off
Turning the edge off on screen turned nothing off: the barriers were
created at boot and the flag was never read again. The reconciler, in
turn, asked the other computer to hand the mouse back, believed it
without checking, and repeated, seven times in two minutes. Now it checks
that the device really left and, after three attempts with no effect, it
turns itself off and says why in the log.

## 2026-11-04 · "View screen" for the Mac uses ours, not RDP
Clicking wakes the other machine's host on its own and opens our window.
The picture was fixed (it was scaled wrong), and it gained a LAN bitrate
and a keyframe every 2 s so that errors heal. Windows stays on RDP until
there is capture there.

## 2026-11-04 · Our own remote screen: keyboard and mouse go back
The loop is closed: the client's input reaches the host and is applied.
What is left is switching the "View screen" button to call ours instead
of RDP.

## 2026-11-04 · Our own remote screen: you can SEE Ubuntu on the Mac
The picture is captured, encoded by the GPU, crosses the network and is
decoded on the M1 at 72 fps. What is left is sending keyboard and mouse
back, to use it and not only look.

## 2026-11-04 · Our own remote screen: the picture reaches the Mac
Captured on Ubuntu, encoded by the GPU and sent over QUIC: 77 fps at
1440p, 1.7 Mbps, no frame lost. What is left is drawing it on screen and
sending keyboard and mouse back.

## 2026-11-04 · Our own remote screen: the first half is ready
Wayland capture and GPU encoding working: 1440p at 75 fps in 6.4 ms per
frame, 5.5 Mbps. What is left is the transport and the Mac client.

## 2026-11-04 · Edge off, the keyboard becomes the leader
The devices switched machines on their own. The edge stepped aside and
switching is now the Easy-Switch button; the reconciler now follows the
keyboard, not the mouse, so it does not undo what you just did.

## 2026-11-04 · Build version in the footer
An incremental number and the commit, the same on all three machines when
they are up to date: a hash alone says they differ, not which one is
behind.

## 2026-11-04 · Task manager
Processes of the three machines, sorted by CPU, with a two-step kill. The
CPU unit is stated: a percentage on Unix, accumulated seconds on Windows.

## 2026-11-04 · Machine layout on the grid
Drag each machine to where it sits on the desk; the edges come from
there. A neighbour that cannot receive devices shows struck through, with
the reason.

## 2026-11-04 · Windows stopped deleting the daemon
Defender removed the daemon's executable as a threat. The binary now
arrives signed, with a timestamp.

## 2026-11-03 · Each OS's native remote screen
A "View screen" button on the panel, with options for keyboard and mouse,
sound and microphone, each one disabled when the client cannot do it.

## 2026-11-03 · File browser
Lists folders on the three machines, locally or over SFTP. Read only.

## 2026-11-03 · The Mac joined
A third machine on the panel: status, files and screen. With no Logitech
receiver, it does not get keyboard and mouse yet.

## 2026-11-03 · Panel and daemon on Windows
Before, it only existed on Ubuntu. Now it is the same binary on both, with
an icon in the Start menu and in the tray.
