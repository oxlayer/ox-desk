# Verify screen, mouse and keyboard on the Mac

Three things can be broken and give the **same** symptom ("it didn't
work on the Mac"): the Screen Recording permission, the Accessibility
permission, and the host simply not running. This recipe separates the
three in ~1 min.

## 0. The host shuts down by itself — that's on purpose

`RunAtLoad false` + idle exit after 20 s without a client, so as not to
spend battery capturing for nobody. **Two of my tests failed because of
this** before I noticed: `input-send` by hand skips the step that
`logid` does by itself.

Waking it up is what `acordar_host` does (logid/src/screen.rs), and it
is idempotent:

```bash
ssh mac 'pgrep -x logistream-host >/dev/null 2>&1 ||
         launchctl kickstart "gui/$(id -u)/ai.logi.logistream-host" >/dev/null 2>&1;
         sleep 1; pgrep -x logistream-host >/dev/null && echo vivo || echo morto'
```

From the tray or the panel it isn't needed: `mandar` already calls this
before opening the KVM.

## 1. Does the screen open?

```bash
ssh mac 'launchctl kickstart -k gui/$(id -u)/ai.logi.logistream-host; sleep 7
         tail -8 ~/.local/state/logistream-host.log'
```

Passing means seeing the four lines:

```
  avfoundation: tela 1, 3360x2100, UYVY422
  h264_videotoolbox pronto, 28 Mbps alvo
  escutando em 0.0.0.0:9777
  acessibilidade concedida: teclado e mouse serao injetados
```

Stopping at `abrindo a tela 1...` = **Screen Recording is missing** for
this binary. No `acessibilidade concedida` = Accessibility is missing.
Without TCC, AVFoundation **hangs instead of failing**, so the last log
line becomes an ffmpeg message about pixel format — which has nothing to
do with the problem. See [[../rules/graphical-session]].

## 2. Does the video arrive?

```bash
cargo run --release --bin logistream-sink -- mac-host.local:9777 5
```

Expected: ~46 fps, `quadros perdidos: 0`.

## 3. Does the mouse arrive?

The probes in `mac/sondas/` compile in 1 s and exist because **"it looks
like it didn't move" is not a measurement**:

```bash
scp mac/sondas/*.swift mac:/tmp/
ssh mac 'cd /tmp && swiftc -O -o cursor cursor.swift && swiftc -O -o tecla tecla.swift'
```

```bash
echo "antes:  $(ssh mac /tmp/cursor)"
cargo run --release --bin logistream-input-send -- mac-host.local:9778 --rel 20
echo "depois: $(ssh mac /tmp/cursor)"
```

`--rel 20` = 20 × +10 px. Passing is X exactly +200 and Y untouched.
Port **9778**: video and input are separate ports (`port + 1`).

## 3b. Does the mouse arrive at the RIGHT place?

`--rel` doesn't answer that: it accumulates deltas and never goes
through the space conversion. It once said "exactly +200 px" with the
absolute path broken — see [[../rules/coordinate-spaces]].

The case that reveals it is the **middle** of the frame. The corner
doesn't work: the system clamps the pointer at the edge and the error
stays hidden behind the clamp.

```bash
for p in "0 0" "3359 2099" "1680 1050" "840 525"; do
  ssh mac 'pgrep -x logistream-host >/dev/null || launchctl kickstart "gui/$(id -u)/ai.logi.logistream-host"; sleep 2'
  cargo run --release --bin logistream-input-send -- mac-host.local:9778 --abs $p
  echo "quadro ($p) -> cursor $(ssh mac /tmp/cursor)"
done
```

Expected on a Retina of 3360×2100 pixels / 1680×1050 points — half of
everything:

```
quadro (0 0)         -> cursor 0 0
quadro (3359 2099)   -> cursor 1679 1049
quadro (1680 1050)   -> cursor 840 525
quadro (840 525)     -> cursor 420 262
```

> **Wake the host inside the loop.** Without it, it dies from idle in
> the middle of the test battery and the following readings repeat the
> last value left on screen — four identical lines that look like a
> result and aren't. It happened to me in this very recipe.

## 4. Does the keyboard arrive?

```bash
ssh mac '/tmp/tecla > /tmp/tecla.out 2>&1 &' & sleep 1
cargo run --release --bin logistream-input-send -- mac-host.local:9778 --segurar 30
sleep 4; ssh mac 'cat /tmp/tecla.out'
```

`A: PRESSIONADA` is passing. `30` is the evdev code for A; the probe
looks at `kVK_ANSI_A = 0`, which is the macOS code — the translation is
what's being tested.

## Did the permission survive the rebuild?

This is the question that cost the most. An ad-hoc binary ties TCC to
the cdhash, which changes on every build; a stable identity ties it to
the designated requirement, which doesn't change. To prove it:

```bash
ssh mac 'cd ~/logi/logistream && touch src/bin/host.rs &&
         export PATH=/opt/homebrew/bin:$PATH PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig &&
         ~/.cargo/bin/cargo build --release --bin logistream-host 2>&1 | tail -1 &&
         install -m755 target/release/logistream-host ~/.local/bin/ &&
         ~/.local/bin/assinar.sh ~/.local/bin/logistream-host'
```

Then repeat step 1. If it opens, it survived.

**Don't run `criar-identidade.sh` again.** It refuses precisely because
of this: swapping the certificate invalidates the grant and costs
another trip to the Mac's Settings. That's what happened to me in the
middle of this work.

## Black frame with a perfect count

On 2026-11-07 the host was sending 59 fps of black at 1920×1440. Index
`1` had become the iPhone camera, because `avfoundation` counts cameras
before the screen. Since then it opens `Capture screen 0` by name. Check
the frame's size and luminance, not just the count.

About the wizard saying "falta" (missing) with the toggle on: a child of
`logid` answers with `logid`'s permissions. That's why the question goes
out as a launchd job.

## See also

- [[deploy]] — the `mac: assinar o host` step fails if the binary stays ad-hoc
- [[../rules/graphical-session]] — why none of this starts over plain SSH
- [[../rules/verify-what-arrived]] — why the host log is the proof
