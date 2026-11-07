# Prototypes

Investigation code that **proved** a path before it became Rust. Kept
because the value is in what it established, not in the code.

| File | What it proved |
|---|---|
| [`border-detect.py`](border-detect.py) | `org.gnome.Mutter.InputCapture` delivers the edge event and gives the pointer back, without libei — while the equivalent portal accepts `CreateSession` and never answers. See [`../specs/005`](../specs/005-input-switch.md#detecção-de-borda-mutter-direto-não-o-portal). |

Run: `/usr/bin/python3 border-detect.py` — needs `python3-gi` and the
**system** python (the linuxbrew one doesn't have `gi`).
