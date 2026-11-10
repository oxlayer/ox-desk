//! How long to wait before asking the graphical session again.
//!
//! Spec: ../../specs/001-control-plane.md
//!
//! The session is not a boolean, it has a timeline. `oxdesk.service` starts
//! with the user session, so on a cold login the daemon can be up and
//! serving before GNOME Shell has published the D-Bus names it depends on:
//! `org.kde.StatusNotifierWatcher` for the tray (published by the
//! AppIndicator extension, which loads after the shell) and
//! `org.gnome.Mutter.RemoteDesktop` for the clipboard.
//!
//! Measured on 2026-11-10: both failed with `ServiceUnknown` at 12:20:00,
//! the same second the service started, and neither asked again. Thirty
//! minutes later the watcher was there with four other icons on it, and
//! oxdesk still had no tray. `systemctl --user restart` brought both up at
//! once -- which is the definition of a bug that should not need a person.

use std::time::Duration;

/// Delay before retry number `attempt`, counted from zero.
///
/// Fast and then slow, because the two reasons a session name is missing
/// have very different timelines. The usual one is the cold login above,
/// where the name appears a second or two later -- waiting half a minute
/// there would leave the tray missing on every boot. The other is a machine
/// that genuinely has no tray and no Mutter, where asking forever is fine
/// as long as it is cheap.
///
/// 1, 2, 4, 8, 16, then 30 s for as long as the daemon lives.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn retry_delay(attempt: u32) -> Duration {
    const CAP_S: u64 = 30;
    // `checked_shl` and not `1 << attempt`: a daemon up for weeks reaches an
    // attempt count that overflows the shift, and in release mode that wraps
    // to zero -- turning the retry into a busy loop on the machine least
    // able to tell anyone about it.
    let seconds = 1u64.checked_shl(attempt).unwrap_or(CAP_S).min(CAP_S);
    Duration::from_secs(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_waits_are_short_because_a_cold_login_is_seconds_away() {
        assert_eq!(retry_delay(0), Duration::from_secs(1));
        assert_eq!(retry_delay(1), Duration::from_secs(2));
        assert_eq!(retry_delay(2), Duration::from_secs(4));
        assert_eq!(retry_delay(4), Duration::from_secs(16));
    }

    #[test]
    fn it_stops_growing_at_half_a_minute() {
        assert_eq!(retry_delay(5), Duration::from_secs(30));
        assert_eq!(retry_delay(9), Duration::from_secs(30));
    }

    #[test]
    fn a_long_lived_daemon_does_not_overflow_into_no_wait_at_all() {
        for attempt in [31u32, 32, 63, 64, 1000, u32::MAX] {
            assert_eq!(
                retry_delay(attempt),
                Duration::from_secs(30),
                "attempt {attempt} must still wait"
            );
        }
    }
}
