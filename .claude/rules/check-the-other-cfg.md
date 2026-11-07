# Code for another OS compiles here — switch the `cfg` and check

`setup_mac.rs` and `setup_win.rs` only enter the build on their own
platform (`#[cfg(target_os = ...)]` in `main.rs`). The consequence is
that **editing the Mac code from here proves nothing**: `cargo build`
passes green because the file wasn't even read.

For Windows this was never a problem — there is a cross target:

```bash
cargo check --target x86_64-pc-windows-gnu --manifest-path logid/Cargo.toml
```

For the Mac there is no SDK here. But most of these modules are pure
`std`: what talks to the system is `Command::new("/usr/bin/security")`,
`launchctl`, `codesign` — strings, not native API. So you can type-check
by switching the `cfg` for a minute:

```bash
cp logid/src/main.rs /tmp/main.rs.bak
sed -i 's/#\[cfg(target_os = "macos")\]\nmod setup_mac;/#[allow(dead_code)]\nmod setup_mac;/' logid/src/main.rs
cargo check --manifest-path logid/Cargo.toml
cp /tmp/main.rs.bak logid/src/main.rs
```

That is how the `setup_mac.rs` of [`015`](../../specs/015-ota.md) was
verified with the MacBook offline the whole day: zero errors, zero
warnings. Had there been a `?` in the wrong place or an extra argument
in a call, I would have found out **after** sending the deploy and
waiting seven minutes of remote compilation — and the deploy would say
`FALHOU` on a line that explains nothing.

## What this does NOT cover

- Real native API (`CoreGraphics` FFI, `getuid`): that doesn't compile
  here, and if the module has a lot of it, the trick doesn't work.
- Behavior. Type-checking says the call exists, not that `launchctl`
  accepts that argument.

In other words: "don't claim it works on the Mac without having run it
there" still holds. What switching the `cfg` eliminates is the silliest
class of error, which is exactly the one that costs the round trip.

## See also

- [[verify-what-arrived]] — compiling is not arriving
- [[graphical-session]] — what isn't born over SSH, and therefore can't be tested from here
