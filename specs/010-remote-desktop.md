# 010 — Our own remote screen (`logistream`)

User request, after seeing the dependency on the Windows App:
*"I want our own implementation, maybe based on rustdesk, written in rust
like parsec"* — with **GPU H.264 and UDP+FEC from the start**, and audio,
pairing and NAT later.

## The size of this, stated before starting

It is the codebase of a RustDesk. Per-OS capture, hardware encoding,
transport with controlled loss, per-OS input injection, decoding and
rendering on the client, audio, crypto and pairing. **Weeks, not one
night.** Any other promise would be a lie, and the project has already
paid dearly for optimistic silence.

Until ours is any good, the Windows App stays on the Mac. It goes away
when ours actually replaces it, not when ours exists.

## The terrain, measured (2026-11-04)

This is not a matter of taste — it was surveyed on all three machines:

| | Ubuntu | Windows | macOS |
|---|---|---|---|
| GPU | RTX 3060 | RTX 3050 | Apple Silicon |
| encoder | NVENC (`libnvidia-encode.so.1`, driver 580) | NVENC | — (it is the client) |
| decoder | NVDEC | NVDEC | VideoToolbox |
| capture | `ScreenCast` portal + PipeWire | DXGI Desktop Duplication | — |
| injection | libei/EIS (`border.rs` already connects to it) | `SendInput` | — |
| libav dev | via linuxbrew, 62.28 (no sudo) | — | via brew |

**Both hosts are NVIDIA**, which simplifies things: one encoding path,
not three. The Mac is the client, and decodes with VideoToolbox.

> The linuxbrew ffmpeg came **without nvenc** (`ffmpeg -encoders` lists only
> `libx264` and `h264_v4l2m2m`). That is why encoding talks to NVENC
> **directly**, via `libnvidia-encode`, instead of going through libavcodec —
> which would be one more layer in the hot path anyway.

## Architecture

### Transport: QUIC, not raw UDP

UDP+FEC was the request, and it is what video needs — but writing raw UDP
also means writing handshake, crypto, congestion control and reassembly.
QUIC delivers all four and **is UDP**:

| need | how |
|---|---|
| video with tolerated loss | QUIC **datagrams** (not retransmitted) |
| FEC | Reed-Solomon on top of the datagrams |
| input and control | QUIC **streams** (reliable, ordered) |
| crypto and identity | TLS 1.3, built in |
| congestion | built in (BBR/Cubic) |
| switching networks without dropping | connection migration, built in |

Writing that by hand would be reimplementing QUIC, worse. `quinn` is the crate.

The peer's identity becomes the **TLS certificate**: pairing is trusting a
public key, which is the model the user had already described — *"it shows
up on the other computer to accept and is saved in the keychain"*.

### Layers

```
capture ──> encoder ──> packetizer ──> quinn ──> network
(portal/   (NVENC)     (FEC + RTP-ish)
 DXGI)

network ──> quinn ──> depacketizer ──> decoder ──> render
                   (rebuilds FEC)   (VideoToolbox)  (wgpu/Metal)
                                   ^
                              input <── streams
```

Each layer sits behind a trait, for a concrete reason: the first encoder
that works will not be the last, and swapping NVENC for VideoToolbox or x264
cannot mean rewriting the pipeline.

### What we will NOT write

- **Codec.** H.264 comes from the hardware.
- **Crypto.** QUIC's TLS.
- **NAT traversal in v1.** LAN first; then `quinn` + ICE.

## Stages, with what each one proves

| v | delivers | the unknown it kills |
|---|---|---|
| **v1** | ✅ **DONE** — screen captured, NVENC, file | Wayland capture and NVENC really work |
| **v2** | ✅ **DONE** — arrives, decodes and draws on the Mac | transport and decoding |
| **v3** | ✅ **DONE** — input from the Mac arrives and is applied | the loop closes: it is usable |
| **v4** | FEC, rate control, Windows as host (DXGI) | quality and the second platform |
| **v5** | audio, PIN pairing, NAT | the rest |

Each stage is usable or disposable on its own. None depends on the next
one existing.

## Where it lives

New crate `logistream/`, separate from `logid`. The daemon remains the
control plane — status, files, processes, keyboard switching — and does
not become a video server. `logid` learns to **launch** and **pair**
`logistream`, as it already launches the screen client today.

## The Parsec Virtual Display Adapter is not junk to sweep away

The user wants to remove Parsec once ours works. Fair — but that virtual
display adapter it installs has a function: it gives the machine a
**screen to capture when no monitor is connected**, and allows the
client's resolution instead of the host's.

If the Windows machine there sometimes runs without a monitor, deleting the
adapter takes away a capability we would have to rebuild (a virtual display
driver is serious business, signed, in kernel). The decision waits until v4
is up and we can measure whether it is missed — not until cleanup time.

## v1, half done: NVENC is proven (2026-11-04)

`cargo run --bin nvenc-probe` encodes **synthetic** frames — capture is
another problem, and mixing the two would make the failure of one look
like the failure of the other.

```
NVENC carregado (dlopen de libnvidia-encode)
120 quadros 1920x1080 em 0.77s = 155 fps
por quadro: p50 3.56ms  p95 3.83ms
39.3 Mbps a 60fps
```

And `ffprobe` reads the file: `h264`, profile `High`, `1920x1080`,
`yuv420p`, **120 frames** — the same ones that went in.

Three things this settles:

1. **No SDK and no sudo.** The `moq-nvenc` crate `dlopen`s
   `libnvidia-encode`, which already came with the driver. Nothing to
   install, and on a machine without an NVIDIA GPU the failure happens at
   *open*, not at compile — which is the right behavior for a binary that
   runs on three machines.
2. **3.56 ms per frame** leaves plenty of room in a 16 ms budget
   (60 fps). Encoding will not be the bottleneck.
3. **39 Mbps is the worst case**: the synthetic gradient changes every
   pixel every frame, on purpose. A real screen — still text, still
   windows — compresses an order of magnitude better.

### A bug the test caught that would have bitten in production

The first version wrote a file that `ffprobe` rejected:
`non-existing PPS 0 referenced`. NVENC does not repeat SPS/PPS by default.

In a file that is annoying; **in a stream it is fatal**, because the normal
case for a remote screen is the client joining midway — and without the
headers it has no way to start drawing anything. The config now enables
`repeatSPSPPS`, which resends them on every IDR.

Since there was explicit config anyway, `NV_ENC_TUNING_INFO_LOW_LATENCY`
went in too: a remote screen lives on latency, not on pretty bitrate.

### What v1 still lacks: capture

Wayland capture needs `libpipewire-0.3-dev`, which **is not installed** and
does not exist in linuxbrew — it is the first `sudo apt` this project will
ask for.

Sudo-free alternative: start with **Windows as the host** (DXGI Desktop
Duplication needs no extra library). To be decided.

## v1 closed: real screen, 1440p, at 38% of the budget (2026-11-04)

`cargo run --release --bin v1-record` captures the monitor, encodes with
NVENC and writes `.h264`:

```
formato: 2560x1440 VideoFormat::BGRx
180 quadros 2560x1440 em 2.38s = 75.5 fps
copia:  p50 1.05ms  p95 1.41ms
encode: p50 5.31ms  p95 5.40ms
total:  p50 6.38ms  p95 7.03ms  (orçamento 16.6ms a 60fps)
2.0 MB = 5.5 Mbps a 60fps
```

`ffprobe` reads 180 frames, `h264` High, `2560x1440`, and an extracted
frame shows the real desktop — VS Code, terminal, dock.

### What the numbers say

**6.38 ms out of 16.6 ms.** That leaves 10 ms to packetize, encrypt and
send. The hot path will not be the bottleneck; the network will.

**5.5 Mbps versus the 39 Mbps of the synthetic gradient.** That was the
prediction: a real screen has still text and still windows, and the encoder
lives on that. The synthetic one changes every pixel of every frame on
purpose, to measure the worst case — and the worst case came out seven
times above the real one.

**BGRx along the whole chain.** Mutter delivers BGRx, NVENC consumes ARGB,
and in memory they are the same byte order — the copy is raw, 1.05 ms,
with no color conversion on the CPU. Had there been a conversion, it alone
would have eaten the budget.

### Capture does not go through the portal

`org.gnome.Mutter.ScreenCast`, the private API the portal itself uses
underneath. The `ScreenCast` portal requires **interactive approval** — it
would open a dialog on the screen of the captured machine, which is exactly
the machine where nobody may be sitting.

Same precedent as the border, which talks to `Mutter.InputCapture` directly
because the equivalent portal accepts `CreateSession` and never answers.

### What v2 still needs

Transport already has a skeleton and a test (`net.rs`: slicing, reassembly,
out of order, incomplete frame counted). What is missing is wiring the ends
together and writing the Mac client — decode with VideoToolbox and draw.

## v2, half done: the screen reaches the Mac (2026-11-04)

`host` on Ubuntu, `sink` on the Mac, over the LAN:

```
770 quadros inteiros em 10.0s = 77.0 fps
1989 datagramas, 1.7 Mbps
quadros perdidos (chegaram incompletos): 0
```

The file that arrived there has 770 `h264` High `2560x1440` frames, and an
extracted frame shows the Ubuntu desktop. On loopback, where the clock is
the same, the **capture → reassembled frame** latency is **p50 6.6 ms**.

### Two bugs, and the second was invisible

**Address family.** `ubuntu-host.local` resolves to IPv4 and IPv6, the
Mac returns IPv6 first, and the client socket was bound to `0.0.0.0` —
quinn rejects it with *"invalid remote address"*. The local socket is now
created from the destination's family. It is the third time a name with
two families has bitten this project (see [`002`](002-ssh.md)).

**The connection driver was not running.** This one was ugly: the host
announced *"80 fps, 0 rejected"* and the receiver saw **zero**.
`send_datagram` only **enqueues**; what puts it on the wire is the
connection driver, which is a tokio task. The send loop blocked the thread
in `recv_timeout` on a `std` channel, and on a `current_thread` runtime that
prevents the driver from running.

Two lessons stay: the channel between capture and network is a **tokio**
one, and a `send_datagram` failure is no longer swallowed — it was the
silent `break` that made the host report success while nothing went out.
Same pattern as `langfuse` and `backfill_gate` in the other repo: the
silent no-op is worse than the loud error.

### The cross-machine latency measurement is wrong, and that is stated

Between Ubuntu and Mac the number comes out `0.0 ms` because the clocks
differ and the subtraction saturates. The label warns about it. Measuring
for real requires a round trip (the receiver returns the timestamp and the
host computes), and that comes together with the v3 input channel — which
needs the return path anyway.

### What is left to close v2

Decode with VideoToolbox and draw in a window on the Mac. The transport
already delivers the whole frame, in order, without loss.

## v2 closed: it decodes on the M1 (2026-11-04)

`view` on the Mac against `host` on Ubuntu:

```
1080 quadros decodificados em 15s  (~72 fps)
host: 79 fps, 1.5 Mbps, 0 recusados
```

Decoding by brew's `libavcodec`, color conversion by `swscale`, window by
`winit` + `softbuffer`.

### Decisions worth explaining

**Network and decoding on one thread, drawing on the other.** The macOS
window loop owns the main thread and cannot block; decoding there would
freeze the interface on every frame.

**Between the two there is a slot, not a queue.** A new frame overwrites
the one not yet drawn. A queue would only accumulate delay — and on a remote
screen an old frame is worth nothing, it has already passed. Same reason as
the shallow channel on the host side.

**The window wakes up by generation, not by timer.** `ControlFlow::Wait`
plus a counter that goes up with every frame: without it the interface
would spend CPU asking whether anything arrived.

**`ffmpeg-next` is pinned to macOS in the config.** The two machines have
different libavcodec versions from brew (61 on the Mac, 62 here), and
pinning per platform solves it without pretending they are the same — the
Linux host does not decode anything.

### What is still CPU work

The YUV→BGRA conversion in `swscale` and the copy into the window buffer.
At 1440p that is a few milliseconds per frame that **VideoToolbox** (and a
YUV shader in `wgpu`) eliminate. It is in v4, together with FEC and rate
control — first make it work, then make the render direct.

### What is missing to replace RDP

v3: sending keyboard and mouse from the Mac back to the host. Without that
you can **see** the machine, not use it.

## v3 closed: input arrives and Mutter accepts it (2026-11-04)

```
input ligado (stream /org/gnome/Mutter/ScreenCast/Stream/u11)
input #1: Mouse { x: 1200.0, y: 700.0 }
...
input #7: Rolagem { dx: 0.0, dy: 15.0 }
```

Seven events, no rejection — the cursor drew a square on the Ubuntu screen.

### `org.gnome.Mutter.RemoteDesktop`, and the choreography it imposes

The **input** session must be created before the **screen** one: its
`SessionId` is what links them, and without the link
`NotifyPointerMotionAbsolute` has no coordinate space to map into.

But on D-Bus the session belongs to the **connection that created it** — a
second connection calling `Notify*` on it is rejected. And the one injecting
is the network thread, not the one talking to PipeWire. Hence the dance:

```
network: create input  -> send SessionId ----------> main
main:    create linked screen, RecordMonitor
         -> send the stream path ----------------> network
network: Start()  (starts both)
main:    receive PipeWireStreamAdded and continue
```

Inside the network thread, everything shares the **same** connection via
`Rc`, and the tasks run in a `LocalSet` — `tokio::spawn` would require
`Send`, which the D-Bus connection is not.

### Input goes by stream, video by datagram

Video tolerates loss; input does not. Dropping a *"key released"* leaves the
key stuck on the host, and a *"button released"* leaves the mouse dragging
by itself. That is why input uses a QUIC stream — reliable and ordered.

The reader accumulates bytes before interpreting: a stream does not respect
message boundaries, and a half-interpreted event would become a wrong
command on someone's machine. Locked down by a test that feeds **one byte
at a time**.

### Physical key, not character

The client sends the key's **position** (evdev keycode), not the character.
Sending the character would let the client's layout rule the host: pressing
a key on an ABNT2 keyboard would turn into something else on a machine with
a US layout. With the position, each machine applies its own layout — which
is how a real keyboard behaves.

### What is missing to retire RDP

Nothing in the protocol: what is missing is **stitching**. The "Ver a tela"
("View the screen") button in the panel still calls an RDP client; it needs
to call ours. And the host needs to start by itself, as a service, instead
of being launched by hand.

## v4, half done: Windows became a host (2026-11-04)

Windows serves its own screen over our protocol. Measured from Ubuntu, with
`logistream-sink` against `windows-host.local:9777`:

| | |
|---|---|
| resolution | 2560×1440, `yuv420p`, valid H.264 (`ffprobe`) |
| frames | 174 whole in 10 s, **zero** lost |
| bandwidth | 1.8 Mbps with the screen nearly still |
| keyframes | 3 in 159 frames — one on connect, the GOP ones after |

### Initialization order is not a detail

Holding the DXGI duplication while CUDA initializes **hangs the process**.
The log stopped at `DXGI: monitor 0, 2560x1440` with 0.2 s of CPU and stayed
there forever; the same NVENC probe, alone, ran at 111 fps in the same
interactive session. Two separate probes (session 0 and session 1) ruled
out "it's the session" as the explanation.

Hence the split into `Monitor` (opens the display, reads size and origin)
and `Tela` (starts duplicating): encoder first, duplication after. As a
bonus it is a better design — if NVENC is not available, we fail before
taking over someone's screen.

### Interactive session: not optional

`SSH` lands in session 0, and there Desktop Duplication does not exist and
`SendInput` reaches no desktop at all. The process **would start without
failing** and deliver a screen nobody is looking at. That is why the Windows
host starts via a scheduled task with `/it`, woken by `schtasks /run` — the
same mechanism that brings up `logid`.

### The cursor does not come in the image

DXGI delivers the screen without the cursor (it comes separately, as shape +
position). We do not draw it for now, and that bothers less than it seems:
since the client sends **absolute** position, the cursor of the viewer's
machine sits exactly over the remote point, with zero latency. What is
missing is the *shape* (text bar, resize arrows). Parsec does the same, for
this reason.

On Linux it is different: we ask Mutter for `cursor-mode: 1` and it embeds
the cursor in the frame. The inconsistency is known and accepted.

> Superseded twice since. First by `cursor.rs`, which composes an arrow into
> the frame for the KVM case. Then by `cursor_win.rs`, which draws it on the
> desktop itself -- see *The cursor nobody drew* at the end of this file.

### The round trip through the CPU is deliberate

The right thing would be to register the D3D11 texture directly in NVENC
(`NV_ENC_DEVICE_TYPE_DIRECTX`) and never leave the GPU. We do staging +
`Map` + upload to CUDA because it is **exactly** the path the Linux host
already takes (PipeWire delivers in CPU memory) — so there is a single
encoder. Changing this is a localized optimization, not an architecture
change.

### What is missing to retire RDP

Only the **viewer** outside macOS. `logistream-view` decodes with
`ffmpeg-next`, and neither Ubuntu nor Windows has `libavcodec` installed.
Meanwhile:

| from → to | protocol today |
|---|---|
| Mac → Ubuntu | **ours** |
| Mac → Windows | **ours** |
| Ubuntu → Windows | RDP (viewer missing here) |
| Windows → Ubuntu | RDP (viewer missing there) |

Two ways to close it: `libavcodec-dev` via apt, or **NVDEC** — both
machines have an NVIDIA GPU and `moq-nvenc` already exposes the decoder's
entry points. NVDEC is more work and drops the system dependency; it is the
path that fits the rest.

## The Mac host: the design decided by measurement (2026-11-04)

The Mac is the last gap -- it watches the other two and does not serve its
own screen. Before writing the pipeline, I measured what the machine offers:

```
libavdevice: linkado e registrado
encoder h264_videotoolbox: existe
encoder libx264: existe
entrada avfoundation: existe
```

That decides the design: **capture via `avfoundation` and encoding via
`h264_videotoolbox`, with no new FFI.** The alternative was raw
ScreenCaptureKit and VideoToolbox -- hundreds of lines of Objective-C
bridging to reach the same place, and brew's ffmpeg is already installed
because it is what decodes in the viewer over there.

`avdevice` went into the `ffmpeg-next` features only on macOS. Linux and
Windows do not need it: they capture via PipeWire and DXGI.

### What is still missing, and the friction that comes with it

- Pipeline: `avfoundation` -> `h264_videotoolbox` -> `serve::serve_video`. The
  network loop and the rate cap are the same as for the other two hosts.
- Input: Core Graphics `CGEvent`, which **requires the Accessibility
  permission** granted by hand in System Preferences. It cannot be worked
  around in code, and it is the kind of thing that, without warning, shows
  up as "the keyboard doesn't work" with no error at all.
- The cursor: `avfoundation` has an option to draw the cursor
  (`capture_cursor`), unlike DXGI. So the Mac can behave like Linux
  (embedded cursor) instead of like Windows.

### An API trap found along the way

`format::list()` existed to enumerate demuxers and **was removed in
ffmpeg 5**; the crate only exposes it under `cfg(not(feature = "ffmpeg_5_0"))`.
The current path is to look it up by name with `av_find_input_format`, which
is what `ffmpeg -f avfoundation` itself does. A non-null pointer proves two
things at once: the device is compiled into the library and `avdevice` was
registered -- without the registration, the name does not resolve.

## "Terrible quality" on Ubuntu -> Windows was three bugs of ours (2026-11-04)

The user compared the two paths and only one was bad: Mac -> Windows
(decoded by `swscale`) was great; Ubuntu -> Windows (our GPU path) was, in
his words, terrible. Different path, same network, same screen: the problem
was our code, not the bandwidth.

Three independent causes, all found by measurement after the comparison
pointed at the guilty path.

### 1. The bitrate never left the floor (nvenc.rs)

`pixels * 60 / 1000` gives 221 kbps at 1440p, so `.clamp(8_000_000, ..)`
returned **8 Mbps at any resolution** -- while the comment right above it
promised ~15 at 1440p and ~34 at 4K. All three hosts encoded at 8 Mbps for
weeks.

It only showed up because the Mac's Retina screen (3360x2100) also
announced "8 Mbps alvo" ("8 Mbps target") and the number was absurd enough
for me to redo the math. **The Windows host never printed the target** --
that is why the bug survived there. Today all three print it: a number
nobody sees is a number nobody checks.

### 2. The VBV measured the average rate and cancelled its own ceiling (nvenc.rs)

    maxBitRate    = alvo * 2     // "may spend double when everything changes"
    vbvBufferSize = alvo / 60    // but the frame can't exceed alvo/60 bits

Two lines that contradicted each other. The `maxBitRate` headroom could
never be spent because the VBV, sized by the average, limited any frame to
16 KB. Scrolling a page is exactly when the encoder needs to spend -- and
that was exactly when it could not, so it raised the QP up to `maxQP`. That
came out as blur **in motion**, which is where the person is looking.

The proof had been in the log all along: a 120 KB keyframe against a 16 KB
VBV. Now the VBV measures one frame of the ceiling; at 1440p the per-frame
budget goes from 16 KB to 60 KB.

| screen | target | ceiling | budget/frame |
|---|---:|---:|---:|
| 1080p | 8.3 Mbps | 16.6 | 33.8 KB |
| 1440p | 14.7 Mbps | 29.5 | 60.0 KB |
| Retina | 28.2 Mbps | 56.4 | 114.8 KB |
| 4K | 33.2 Mbps | 66.4 | 135.0 KB |

### 3. The conversion aliased and was also the bottleneck (decode_nv.rs)

`nv12_to_bgra` sampled by nearest neighbor, with a comment of mine saying
bilinear was "the next step if it bothers anyone". It did: shrinking 1440p
into a window aliases every letter, and only on the GPU path -- the
`swscale` of the other path was already bilinear. Side by side the
difference jumps out.

**Fixing that alone would have made it worse.** Bilinear quadruples the
luma reads, so I measured before trusting it: **28.25 ms per frame, ceiling
of 35 fps** -- and the user's viewer showed exactly **27 fps drawn**. The
network delivered 60 and drawing held it back. Nobody had measured the
conversion because it does not look like a bottleneck; it is just a loop.

Two fixes in the same loop:

- the column table does not depend on the row, and was being rebuilt 1232
  times per frame;
- output rows are independent of each other, so the frame is split into
  bands per core.

28.25 ms -> **2.76 ms**, ceiling of 362 fps. It became a budget test
(`conversion_fits_the_budget`, 12 ms ceiling) so it does not come back.

### The lesson

The three bugs were invisible individually and visible together, and all
three were found by the same move: **comparing two paths that should give
the same result**. The Mac served as the control. Without it I would have
blamed the network.

The second pattern: two of the three survived because the number that
would have exposed them was not printed anywhere (the target on Windows) or
had never been measured (the cost of the conversion). Instrumenting cost
minutes and pointed straight at them.

## The cursor nobody drew (2026-11-07)

The receiver came off the Windows machine, and with it the cursor. Not from
the video -- from the **monitor**. The person drove the machine from the
other side through the LAN KVM, looked at its physical screen, and saw
hover effects answering with nothing to say where the pointer was.

### Measured, not guessed

With no pointing device installed, Windows stops drawing the pointer.
`GetCursorInfo` says it plainly: `flags = 0`, `hCursor = 0` -- **and the
position is still correct**. `GetCursorPos` answers whether or not anything
is drawn, which is exactly what we needed. The ghost devices confirm the
other half: `Get-PnpDevice -Class Mouse` lists thirteen
`HID-compliant mouse`, every one of them `Unknown` -- that is, absent.

`SendInput` moving the position does not bring the cursor back. Injection
changes where the pointer *is*, not whether the OS paints it.

### Two paths, and only one of them was covered

| Who is looking | Where the arrow has to be | Who draws it |
|---|---|---|
| Viewer on another machine | inside the captured frame | `cursor.rs`, composed into the staging texture |
| **Somebody in front of the Windows monitor** | **on the desktop** | **`cursor_win.rs`, a layered window** |

The second row was empty, and it is the one that hurt. There is no frame of
ours in that path at all: the host log of the session that triggered this
shows `input connected` plus eight `MouseRel` and **not one
`sending frame`** -- nobody was watching a video, so composing into the
frame drew for an audience of zero.

### Why a window and not a bigger composite

A layered window is a real desktop window, so DXGI captures it like any
other -- one mechanism serves the physical monitor *and* every viewer. The
cost is that the two would now both draw, so `capture_win` asks
`cursor_win::drawing()` and backs off while the overlay is up. Two arrows
on the viewer would be worse than none: you would not know which one is the
pointer.

Four `WS_EX` flags make it a cursor instead of a window: `LAYERED` for
per-pixel alpha, `TRANSPARENT` so clicks reach what is underneath,
`NOACTIVATE` so it never steals focus from what the person is typing into,
`TOOLWINDOW` to stay out of Alt-Tab and the taskbar.

Both paths read the **same** `ARROW` table. `builtin_arrow` renders it as
DXGI's monochrome AND/XOR, `arrow_bgra` as premultiplied BGRA for
`UpdateLayeredWindow`. One picture, two formats -- a second table would let
the cursor on the desktop and the cursor in the frame drift apart.

### It only draws while someone is driving

A fullscreen game hides the pointer **on purpose**, and an arrow we paint
over it is us intruding. So the lifetime is the input session and not the
host's: `driving(true)` when an input connection arrives, `false` through a
`Drop` guard when it ends -- `receive_input` leaves through several doors,
and the one that forgot would leave an arrow stuck on the screen forever.
The counter is not a bool because there are two doors for input (the viewer
sends over the video connection, the KVM over the input port); with a bool,
whichever hung up first would turn off the arrow of whoever was still
typing.

### Knobs and limits

- `LOGI_CURSOR_ESCALA` repeats each pixel N times. Default 1, which is the
  classic Windows arrow (12x19 of visible arrow inside its 32x32 bitmap).
  It is an env var and not a config field because it is tuned by eye, on
  the machine, and a rebuild to discover that 1 was too small would be
  absurd.
- The shape is always the arrow. The system's text bar and resize arrows
  only exist when the system has a cursor -- and then it is drawing it
  itself and we are hidden.
- Still not covered: a genuinely locked session. The secure desktop belongs
  to another desktop and neither our window nor DXGI reaches it (specs/014).
