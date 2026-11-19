// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Free space on each machine's system drive, and when it counts as low.
//!
//! Spec: ../../specs/027-disk-space.md
//!
//! On 2026-11-18 the Windows machine's system drive reached 5 MB free. A
//! deploy stopped with "not enough space", installs stalled, and an agent's
//! own temporary writes failed, while the panel showed that machine's RAM
//! and CPU every few seconds and said nothing about its disk. It was the
//! fourth time in two months on the same machine.
//!
//! The numbers come with the rest of the machine's detail (`probe`); this
//! module only decides what they mean, so the card, the summary line and the
//! log all say the same thing.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    /// Time to clean up, before it fills.
    Low,
    /// Things on that machine start failing.
    Critical,
}

impl Level {
    fn word(self) -> &'static str {
        match self {
            Level::Ok => "back to normal",
            Level::Low => "low",
            Level::Critical => "almost full",
        }
    }
}

/// Low below 10% of the drive, critical below 3%, each capped (10 GB and
/// 2 GB) so a big drive is not called low with hundreds of GB free.
///
/// On the 112 GB drive of 2026-11-18 that is low under 10 GB and critical
/// under 2 GB: it was critical at 5 MB, and still low at 9.4 GB after the
/// first cleanup, which was right -- a single install filled it again a week
/// earlier.
///
/// `None` when the numbers cannot be a drive: a reading that failed must not
/// show up as a level.
pub fn level(free_gb: f64, total_gb: f64) -> Option<Level> {
    if !free_gb.is_finite() || !total_gb.is_finite() || total_gb <= 0.0 || free_gb < 0.0 {
        return None;
    }
    let critical = (total_gb * 0.03).min(2.0);
    let low = (total_gb * 0.10).min(10.0);
    Some(if free_gb < critical {
        Level::Critical
    } else if free_gb < low {
        Level::Low
    } else {
        Level::Ok
    })
}

/// Does going from `before` to `now` deserve a log line? Every change does,
/// and so does the first reading when it is not ok: a daemon that starts on a
/// full drive has to say so, and one that starts on a healthy drive has
/// nothing to say.
fn worth_saying(before: Option<Level>, now: Level) -> bool {
    match before {
        None => now != Level::Ok,
        Some(b) => b != now,
    }
}

/// The last level seen per machine.
static LAST: Mutex<BTreeMap<String, Level>> = Mutex::new(BTreeMap::new());

/// Reads `host`'s numbers, logs when its level changed, and returns the level.
pub fn note(host: &str, free_gb: Option<f64>, total_gb: Option<f64>) -> Option<Level> {
    let (free, total) = (free_gb?, total_gb?);
    let now = level(free, total)?;
    let before = LAST.lock().ok().and_then(|mut m| m.insert(host.to_string(), now));
    if worth_saying(before, now) {
        say!("disk: {host}'s system drive is {}: {free:.1} GB free of {total:.0} GB", now.word());
    }
    Some(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drive of the incident, at each point of that evening.
    #[test]
    fn the_112_gb_drive_of_2026_11_18() {
        assert_eq!(level(0.005, 112.0), Some(Level::Critical), "5 MB free");
        assert_eq!(level(1.0, 112.0), Some(Level::Critical), "1 GB free");
        assert_eq!(level(9.4, 112.0), Some(Level::Low), "after the first cleanup");
        assert_eq!(level(21.0, 112.0), Some(Level::Ok), "after the old VS Code builds went");
    }

    /// The caps: 10% of a 2 TB drive is 200 GB, and 150 GB free is not low.
    #[test]
    fn a_big_drive_is_not_low_with_plenty_free() {
        assert_eq!(level(150.0, 2000.0), Some(Level::Ok));
        assert_eq!(level(9.9, 2000.0), Some(Level::Low));
        assert_eq!(level(1.9, 2000.0), Some(Level::Critical));
    }

    /// Without the caps a small drive would never be low; the percentages
    /// carry it.
    #[test]
    fn a_small_drive_uses_the_percentages() {
        assert_eq!(level(3.0, 32.0), Some(Level::Low), "under 10% of 32 GB");
        assert_eq!(level(0.9, 32.0), Some(Level::Critical), "under 3% of 32 GB");
        assert_eq!(level(4.0, 32.0), Some(Level::Ok));
    }

    #[test]
    fn numbers_that_cannot_be_a_drive_have_no_level() {
        assert_eq!(level(1.0, 0.0), None);
        assert_eq!(level(-1.0, 100.0), None);
        assert_eq!(level(f64::NAN, 100.0), None);
        assert_eq!(level(1.0, f64::INFINITY), None);
    }

    #[test]
    fn a_line_on_every_change_and_on_a_bad_start() {
        assert!(!worth_saying(None, Level::Ok), "a healthy start says nothing");
        assert!(worth_saying(None, Level::Low));
        assert!(worth_saying(None, Level::Critical));
        assert!(worth_saying(Some(Level::Low), Level::Critical));
        assert!(worth_saying(Some(Level::Critical), Level::Ok), "coming back is news too");
        assert!(!worth_saying(Some(Level::Low), Level::Low), "no line on every poll");
    }
}
