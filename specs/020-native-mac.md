# 020 — Native on the Mac, and one dependency fewer

> Written for: whoever writes the FFI, and whoever has to decide whether
> it is worth it.

The Mac is the only machine in this product that links a third-party
media library. Parsec does the same job on the same hardware without one,
and it can because **what we use FFmpeg for on macOS is a wrapper around
two Apple frameworks we could call directly.** This spec says what the
dependency actually buys, what it costs, what replacing it looks like, and
argues the other side — because dropping it reverses a decision that was
made deliberately and for good reasons.

## What FFmpeg does for us on macOS, exactly

Two things, in `stream/src/host_mac.rs` (398 lines) and
`stream/src/decode.rs` (181 lines):

| What | Through | Which Apple framework is underneath |
|---|---|---|
| Screen capture | libavdevice's `avfoundation` input device | AVFoundation |
| H.264 encode | libavcodec's `h264_videotoolbox` | VideoToolbox |
| H.264 decode (Mac as client) | libavcodec | VideoToolbox |

Linux captures through PipeWire with Mutter's ScreenCast API and encodes
with NVENC; Windows uses Desktop Duplication and NVENC. **Neither links
FFmpeg at all.** This is a macOS-only dependency with a macOS-only payoff.

## What it has cost, measured

Not a list of annoyances — each of these consumed a day or produced a
wrong artifact:

| Cost | Evidence |
|---|---|
| A package that built and could not be installed | CI built against the runner's unversioned `ffmpeg` (then 9.0.2, `libavcodec.63`); the MacBook has 7.1 (`.61`), so OTA refused the whole update naming the dylib ([[013-ci-cd]]) |
| A versioned prefix that is not a keg | `/opt/homebrew/opt/ffmpeg@7` on the reference Mac is a hand-made symlink, because macOS 14 is Tier 3 for Homebrew: no bottle, and `brew install ffmpeg@7` fails from source (`make: *** [frameworkinstallapps] Error 2`, measured 2026-11-08) |
| Rewriting what the binary says it needs | `build-binaries.sh` now rewrites five recorded dylib paths with `install_name_tool`, guarded by an inode comparison, so the package pins the versioned prefix. That machinery exists **only** because of this dependency |
| A GPL-3 library inside what we distribute | Homebrew's FFmpeg is GPL-3.0-or-later (`--enable-gpl --enable-version3`), which is what forced the license question ([[018-open-source]], [[../THIRD-PARTY]]) |
| One wizard item, and a package manager in the install path | `ffmpeg` is 1 of the Mac's 9 onboarding items ([[019-one-onboarding]]), and it is the only reason a Mac needs Homebrew at all |

## What the native path is

| Piece | API | Note |
|---|---|---|
| Capture | **ScreenCaptureKit** (`SCStream`, macOS 12.3+) | The supported API. AVFoundation's screen input — what `avfoundation` uses — is the legacy one |
| Encode | **VideoToolbox** (`VTCompressionSession`, `kCMVideoCodecType_H264`) | Hardware, exactly what `h264_videotoolbox` wraps |
| Decode | **VideoToolbox** (`VTDecompressionSession`) | For the Mac as client |
| Pixels | `CVPixelBuffer` / `CMSampleBuffer` | ScreenCaptureKit hands these straight to VideoToolbox, so the conversion we pay for today may disappear |

All three are system frameworks: present on every supported macOS, versioned
by the OS, nothing to install. From Rust this is `objc2` plus the framework
crates, or hand-written FFI for the handful of calls involved.

**What this removes:** Homebrew from the Mac install path, the `ffmpeg@7`
prefix and its hand-made symlink, the `install_name_tool` rewriting in
`build-binaries.sh`, the OTA's dylib check for the Mac, the `ffmpeg`
wizard item, and the GPL-3 library from the distribution.

**What it does not remove:** the license. AGPL-3.0-or-later stays, now by
choice rather than obligation — section 13 is aimed at the relay, not at
FFmpeg ([[018-open-source]]).

## Adversarial review

### 1. "This reverses a deliberate decision, and the reason was good"

`stream/src/bin/mac-probe.rs` exists for one purpose: to answer whether
libavdevice links and registers, and its closing line is
`=> the Mac host ships WITHOUT new FFI: avfoundation + videotoolbox`.
Choosing FFmpeg was choosing *not* to write Objective-C FFI from Rust, and
that bought a working screen path quickly.

*What survives:* the decision was right with the information available and
is wrong now — not because FFI got cheaper, but because the dependency's
cost became visible and is still being paid, in the table above. The
honest framing is "the trade moved", not "that was a mistake".

### 2. "New FFI is the riskiest code in the project"

ScreenCaptureKit is Objective-C with async delegate callbacks, which from
Rust means `objc2` and `block2`, retain/release discipline, and a
`CMSampleBuffer` lifetime crossing a thread boundary into our encoder. A
use-after-free there is a crash in the daemon that watches the screens.

*What survives:* entirely, and it sets the order of work. The encoder goes
first, alone, behind a flag, measured against the current path on the same
frames — because VideoToolbox is a C API with no delegates, and it is half
the win. Capture, the harder half, follows only once the encoder is
boring.

### 3. "The screen works today; this risks latency and quality for zero new features"

True. There is a measured budget in this repo and a conversion test that
already fails on shared CI hardware. A rewrite that lands at 1.4× the
latency is worse than a Homebrew dependency.

*What survives:* the flag and the comparison are not optional. The native
path ships when it is measured side by side on the same machine — fps,
p50/p95 capture-to-frame, and bitrate at the same quality — and not
before. If it loses, the dependency stays and this spec records why,
which is worth more than the attempt.

### 4. "The Mac is one machine out of three; this is a lot of work for a third of the fleet"

Also true, and it is the weakest objection: the Mac is the machine that
**travels**, so it is the one where "needs Homebrew, and the right
Homebrew" is most likely to be the reason the product does not work. It is
also the platform where a new user is most likely to arrive.

### 5. "ScreenCaptureKit needs macOS 12.3+"

It does, and so does its permission model. The reference machine is macOS
14, so this is not a constraint today — but it is a floor to write down
before someone tries to support an older Mac.

## Measured so far (2026-11-08)

**Step 1 passed.** `oxdesk-probe-mac-native`, hand-written FFI, no media
crate:

```
CVPixelBuffer: ok, stride 5120
VTCompressionSession: criada
parameter set 0: 16 bytes, nal_type 7 (SPS), length prefix 4 bytes
parameter set 1: 4 bytes, nal_type 8 (PPS)
callback: 1 chamada(s), OSStatus 0
access unit: 18231 bytes
nal 0: tipo 6 (SEI), 30 bytes
nal 1: tipo 5 (IDR slice), 18193 bytes
```

So the encoder half is settled: `h264_videotoolbox` is a wrapper and the
session opens from Rust with a null specification, which on Apple Silicon
is hardware.

**Step 2's order was changed, and the reason is worth keeping.** The spec
said encoder first, fed from the existing capture. That would have meant a
pixel conversion the final design does not have — the native chain hands a
`CVPixelBuffer` from ScreenCaptureKit straight to VideoToolbox — so the
measurement would have been pessimistic and the plumbing thrown away.
Capture and encode are therefore proved together.

**Step 2 is blocked on a TCC grant, not on code.** `ScreenCaptureKit`
answers `-3801 The user declined TCCs for application, window, display
capture`, and it does so from three places: a fresh probe binary, from
`oxdesk-host --probe-nativo` over SSH, and from `oxdesk-host` launched by
a LaunchAgent **inside the graphical session**. The reason is not the
session: `oxdesk-host --conferir` reports **`gravacao-de-tela: falta`**.
The host simply does not hold the grant.

Which is a finding about the fleet and not about this work: the grant
belongs to the binary, the rename changed `logistream-host` into
`oxdesk-host`, and `scripts/migrate-from-logi/mac.sh` says in its own
header that Screen Recording and Accessibility have to be approved again
by hand after that. Nobody did. **The Mac has not been able to serve its
own screen since the cutover**, and the wizard has been saying so.

One thing already learned in favour of the native path, by accident:
ScreenCaptureKit **returns a loud error** where AVFoundation, per
[[../.claude/rules/graphical-session]], simply *hangs* without the grant —
"macOS is the worst of the three, because it doesn't fail". The native API
fails. That is worth more than it sounds for a wizard that has to tell
someone what is wrong.

## Order of work

1. **A probe, like `mac-probe` was for FFmpeg.** **Done** — see above.
   *Check:* the probe prints the NAL types of one encoded frame.
1b. **Grant Screen Recording to `oxdesk-host`** in System Settings, which
   is also what makes the Mac a screen host again. Only the machine's
   owner can do it ([[019-one-onboarding]], the floor). *Check:*
   `oxdesk-host --conferir` says `gravacao-de-tela: ok`, and
   `oxdesk-host --probe-nativo` prints a frame.
2. **Encoder behind a flag**, same frames in, both paths measured.
   *Check:* a table of fps, p50/p95 and bitrate for both, on the
   reference Mac.
3. **Decoder**, same treatment, for the Mac as client.
4. **Capture with ScreenCaptureKit**, last, because it is the Objective-C
   half. *Check:* a frame decoded from the native path matches the FFmpeg
   path's, and the cursor behaves the same ([[010-remote-desktop]]).
5. **Delete the dependency and everything it dragged in:** `ffmpeg-next`,
   the `FFMPEG_*` declarations and `install_name_tool` rewriting in
   `build-binaries.sh`, the OTA dylib check, the `ffmpeg` wizard item, and
   Homebrew from the Mac install path. *Check:* `otool -L` on the Mac
   binaries shows no `/opt/homebrew` path at all, and a Mac with no
   Homebrew reaches "ready" in the wizard.

## See also

- [[010-remote-desktop]] — the screen stack, and the budgets a rewrite
  has to meet
- [[013-ci-cd]] — the package that built and could not be installed
- [[019-one-onboarding]] — the wizard item this removes
- [[018-open-source]] and [[../THIRD-PARTY]] — the GPL-3 linkage this
  removes
