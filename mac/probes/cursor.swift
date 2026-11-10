// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

// Where the pointer is, in pixels. One line per reading.
//
// It exists because "it looks like it did not move" is not a measurement.
// With the position read before and after a burst of MouseRel, it either
// moved the expected pixels or it did not -- and the difference points to
// the right side (arrived and was refused vs. never arrived).
//
// Needs no permission: reading the cursor position is not TCC.
import CoreGraphics
let p = CGEvent(source: nil)!.location
print("\(Int(p.x)) \(Int(p.y))")
