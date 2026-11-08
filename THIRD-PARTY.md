# Third-party code in what we distribute

The published packages are not only our code. This file says what else is
inside them and under which terms, because the Mac package carries a
GPL-3.0-or-later library and that decides the license of the whole binary.

## FFmpeg — GPL-3.0-or-later — Mac packages only

`oxdesk-host` and `oxdesk-view` on macOS link Homebrew's `ffmpeg@7`
dynamically: capture comes from libavdevice's `avfoundation` and encoding
from `h264_videotoolbox`. Linux and Windows capture through PipeWire and
DXGI and encode with NVENC loaded at runtime, so **their packages carry no
FFmpeg at all**.

Measured on the reference Mac (2026-11-08):

| | |
|---|---|
| Formula | `ffmpeg@7` (Homebrew) |
| License reported by Homebrew | **GPL-3.0-or-later** |
| Build flags | `--enable-gpl --enable-version3` |
| Recorded paths in our binaries | `/opt/homebrew/opt/ffmpeg@7/lib/libav*.dylib` |

Because the distributed Mac binaries link a GPL-3.0-or-later library, the
combined work must be under a GPL-3-compatible license. **That is the
concrete reason this project is AGPL-3.0-or-later** and not the MIT its
crate metadata used to claim: AGPL-3.0 and GPL-3.0 are compatible, MIT
source combined with GPL-3 binaries is not a coherent thing to publish.

FFmpeg's own source is available from the FFmpeg project and from
Homebrew's formula. Anyone receiving our Mac package is entitled to it
under the GPL, and the link belongs in the release notes of any package
that ships linked against it.

## Rust dependencies — permissive

Every crate in the dependency tree is `MIT`, `Apache-2.0`,
`Apache-2.0 WITH LLVM-exception`, `Unicode-3.0` or `Unlicense`, checked
with `cargo tree --format '{l}'` on 2026-11-08. No GPL-2.0-**only**
dependency exists anywhere, which matters because GPL-2.0-only cannot be
combined with AGPL-3.0.

## Electron — MIT — the panel shell

`gui/` is an Electron shell with no logic. Electron is MIT; the Chromium
and Node components it bundles carry their own notices inside the
distributed app bundle.

## Our own split

| Crate | License | Why |
|---|---|---|
| `hidpp` | MIT | HID++ 2.0 over raw HID is useful on its own, to projects that are not this one, and it links nothing GPL. Permissive is what makes that reuse possible. |
| `oxdesk`, `stream`, `ota`, `agent`, `winput` | AGPL-3.0-or-later | The product. `stream` must be GPL-3-compatible because of FFmpeg; the rest follows for coherence, and `oxdesk` is the network-facing part where the AGPL's section 13 actually applies. |

## See also

- [`LICENSE`](LICENSE) — AGPL-3.0 full text
- [`hidpp/LICENSE`](hidpp/LICENSE) — MIT, for that crate only
- [`specs/018-open-source.md`](specs/018-open-source.md) — why AGPL, and
  the open question about a CLA
