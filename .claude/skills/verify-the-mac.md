# Verify screen, mouse and keyboard on the Mac

Three things can be broken and give the **same** symptom ("it didn't
work on the Mac"): the Screen Recording permission, the Accessibility
permission, and the host simply not running. This recipe separates the
three in about a minute.

## 0. The host shuts down by itself — that's on purpose

`RunAtLoad false` + idle exit after 20 s without a client, so as not to
spend battery capturing for nobody. **Two test runs failed because of
this** before the cause was found: running `input-send` by hand skips the
step that `oxdesk` does by itself.

Waking it up is what `wake_host` does (oxdesk/src/screen.rs), and it is
idempotent:

```bash
ssh mac-host 'pgrep -x oxdesk-host >/dev/null 2>&1 ||
              launchctl kickstart "gui/$(id -u)/app.oxdesk.host" >/dev/null 2>&1;
              sleep 1; pgrep -x oxdesk-host >/dev/null && echo alive || echo dead'
```

From the tray or the panel it isn't needed: `send` already calls it
before opening the KVM.

## 1. Does the screen open?

```bash
ssh mac-host 'launchctl kickstart -k gui/$(id -u)/app.oxdesk.host; sleep 7
              tail -8 ~/.local/state/oxdesk-host.log'
```

Passing means seeing these four lines. They were captured while the host
still logged in Portuguese; the English the current host prints is in the
comments:

```
  avfoundation: tela 1, 3360x2100, UYVY422                     # avfoundation: screen ...
  h264_videotoolbox pronto, 28 Mbps alvo                       # h264_videotoolbox ready, 28 Mbps target
  escutando em 0.0.0.0:9777                                    # listening on 0.0.0.0:9777
  acessibilidade concedida: teclado e mouse serao injetados    # accessibility granted: keyboard and mouse will be injected
```

Stopping at `opening screen ...` = **Screen Recording is missing** for
this binary. No `accessibility granted` = Accessibility is missing.
Without TCC, AVFoundation **hangs instead of failing**, so the last log
line becomes an ffmpeg message about pixel format — which has nothing to
do with the problem. See [[../rules/graphical-session]].

## 2. Does the video arrive?

```bash
cargo run --release --bin oxdesk-sink -- mac-host.local:9777 5
```

Expected: ~46 fps, `frames lost (arrived incomplete): 0`.

## 3. Does the mouse arrive?

The probes in `mac/probes/` compile in 1 s and exist because **"it looks
like it didn't move" is not a measurement**:

```bash
scp mac/probes/*.swift mac-host:/tmp/
ssh mac-host 'cd /tmp && swiftc -O -o cursor cursor.swift && swiftc -O -o key key.swift'
```

```bash
echo "before: $(ssh mac-host /tmp/cursor)"
cargo run --release --bin oxdesk-input-send -- mac-host.local:9778 --rel 20
echo "after:  $(ssh mac-host /tmp/cursor)"
```

`--rel 20` = 20 × +10 px. Passing is X exactly +200 and Y untouched.
Port **9778**: video and input are separate ports (`port + 1`).

## 3b. Does the mouse arrive at the RIGHT place?

`--rel` doesn't answer that: it accumulates deltas and never goes
through the space conversion. It once reported "exactly +200 px" while
the absolute path was broken — see [[../rules/coordinate-spaces]].

The case that reveals it is the **middle** of the frame. The corner
doesn't work: the system clamps the pointer at the edge and the error
stays hidden behind the clamp.

```bash
for p in "0 0" "3359 2099" "1680 1050" "840 525"; do
  ssh mac-host 'pgrep -x oxdesk-host >/dev/null || launchctl kickstart "gui/$(id -u)/app.oxdesk.host"; sleep 2'
  cargo run --release --bin oxdesk-input-send -- mac-host.local:9778 --abs $p
  echo "frame ($p) -> cursor $(ssh mac-host /tmp/cursor)"
done
```

Expected on a Retina display of 3360×2100 pixels / 1680×1050 points —
half of everything:

```
frame (0 0)         -> cursor 0 0
frame (3359 2099)   -> cursor 1679 1049
frame (1680 1050)   -> cursor 840 525
frame (840 525)     -> cursor 420 262
```

> **Wake the host inside the loop.** Without it, the host exits from
> idle in the middle of the test battery and the following readings
> repeat the last value left on screen — four identical lines that look
> like a result and aren't. This happened while this recipe was being
> written.

## 4. Does the keyboard arrive?

```bash
ssh mac-host '/tmp/key > /tmp/key.out 2>&1 &' & sleep 1
cargo run --release --bin oxdesk-input-send -- mac-host.local:9778 --hold 30
sleep 4; ssh mac-host 'cat /tmp/key.out'
```

`A: PRESSIONADA` ("A: pressed") is passing; `A: nunca apertada` ("A:
never pressed") is failing; the probe prints those words deliberately
(see the comment in `mac/probes/key.swift`). `30` is the evdev code for
A; the probe looks at `kVK_ANSI_A = 0`, which is the macOS code — the
translation is what's being tested.

## Did the permission survive the rebuild?

This is the question that cost the most. An ad-hoc binary ties TCC to
the cdhash, which changes on every build; a stable identity ties it to
the designated requirement, which doesn't change. To prove it:

```bash
ssh mac-host 'cd ~/oxdesk/oxdesk-stream && touch src/bin/host.rs &&
              export PATH=/opt/homebrew/bin:$PATH PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig &&
              ~/.cargo/bin/cargo build --release --bin oxdesk-host 2>&1 | tail -1 &&
              install -m755 target/release/oxdesk-host ~/.local/bin/ &&
              ~/.local/bin/sign.sh ~/.local/bin/oxdesk-host'
```

Then repeat step 1. If it opens, it survived.

**Don't run `create-identity.sh` again.** It refuses precisely because
of this: swapping the certificate invalidates the grant and costs
another trip to the Mac's System Settings. That happened once, in the
middle of this work.

## Black frame with a perfect count

On 2026-11-07 the host was sending 59 fps of black at 1920×1440. Index
`1` had become an iPhone's camera, because `avfoundation` counts cameras
before the screen. Since then it opens `Capture screen 0` by name. Check
the frame's size and luminance, not just the count.

If the wizard reports a permission as missing while its toggle is on: a
child of `oxdesk` answers with `oxdesk`'s permissions. That's why the
question goes out as a launchd job.

## See also

- [[deploy]] — the `mac: sign the host` step fails if the binary stays ad-hoc
- [[../rules/graphical-session]] — why none of this starts over plain SSH
- [[../rules/verify-what-arrived]] — why the host log is the proof
