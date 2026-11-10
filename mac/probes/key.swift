// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

// Is the A key pressed? Watches for 4s and answers.
//
// Caps Lock is NOT a valid test on macOS: the system handles it in the HID
// layer and a synthetic event does not toggle it, even when it arrives and
// gets translated. It has already measured "does not work" with the whole
// path working. An ordinary key, watched while it is held, is a real
// measurement.
//
// kVK_ANSI_A = 0, which is the macOS code -- the protocol carries evdev,
// where A = 30. Three numberings coexist in this project; see
// `mac_key_from_evdev` in stream/src/input.rs.
//
// The printed answer (`A: PRESSIONADA` / `A: nunca apertada`) stays as is:
// the verify-the-mac skill tells people to look for it.
import CoreGraphics
import Foundation

var seen = false
for _ in 0..<400 {
    if CGEventSource.keyState(.combinedSessionState, key: 0) { seen = true; break }
    usleep(10_000)
}
print(seen ? "A: PRESSIONADA" : "A: nunca apertada")
