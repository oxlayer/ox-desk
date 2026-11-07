# `hidpp`

HID++ 2.0 over raw HID. Used by [`logid`](../logid/) (Linux) and by
[`agent`](../agent/) (Windows) — **a single copy** of the protocol.

It replaced the `solaar`-based backend on Linux. Measured:

| | solaar | direct HID++ |
|---|---|---|
| presence of one device | 5.9 to 15.3 s | **22 to 81 ms** |
| full inventory | 5.9 to 15.3 s | ~1.5 s |
| channel switch, end to end | 105 s | ~9 s |

## Diagnostics

```bash
cargo run --release --bin probe
```

Lists endpoints, opens the receiver and scans the slots with name,
channel and ping. Useful to find the **exact name** the config needs:
HID++ reports `MX Keys Wireless Keyboard`, while solaar said `MX Keys
Keyboard`.

## Decisions that took measurement

**Long report on the usage 2 collection.** The receiver exposes one HID
collection per report type (1 = short `0x10`, 2 = long `0x11`, 4 = very
long). Writing the wrong size to the wrong collection fails with an empty
`hidapi error:`. We always use the long one, to write and read on the
same collection.

**We address the receiver, not the device node.** On Linux each paired
device has its own hidraw node, but it answers on the **receiver's slot
index** (measured: `/dev/hidraw2` answers at `idx=1`), not on `0xFF`.
Since the receiver node answers for all slots, that's the one we open.

**Ping timeout (250 ms) separate from the call timeout (700 ms).** A
present device answers in 26-90 ms; scanning 6 empty slots dropped from
4.4 s to 1.5 s.

**No response to `set_host` is the expected case.** The device switches
channel and the radio drops before the response. Treating that as an
error made the system report failure on a successful switch.
