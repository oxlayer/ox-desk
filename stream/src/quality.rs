//! How many bits per second the encoder may spend.
//!
//! Spec: ../../specs/012-outside-the-lan.md
//!
//! ## Why this became a module
//!
//! The formula was written **twice**: in `nvenc.rs` (Linux and Windows)
//! and again, by hand, in `host_mac.rs`. Two copies of the same math is
//! how the wrong 8 Mbps target survived for weeks -- fixing one and
//! forgetting the other gives no error, it gives silent divergence
//! between machines.
//!
//! ## Why a cap, and why it matters now
//!
//! The target scales with screen area: ~8 Mbps at 1080p, ~15 at 1440p,
//! ~28 on the MacBook's Retina. At home that is bandwidth to spare and
//! the picture looks good.
//!
//! Away from home the bottleneck stops being the LAN and becomes the
//! home's **upload**, which on a residential link is a fraction of the
//! download. Sending 15 Mbps through a 10 Mbps upload fills the router's
//! queue: latency climbs, packets drop, and the screen falls apart --
//! which is worse than a simpler picture delivered on time.
//!
//! ## What this is NOT
//!
//! It is not congestion control. The target is chosen once and does not
//! move by itself; if the link gets worse mid-session, nothing reacts.
//! `quinn` already measures the path's RTT and loss
//! (`Connection::stats()`), and feeding that back to the encoder is the
//! next step -- Parsec's BUD is basically that loop. A hand-picked cap
//! solves "15 Mbps does not fit my upload"; only adaptation solves "the
//! hotel Wi-Fi varies".

use std::sync::OnceLock;

/// Absolute floor and ceiling of the per-area scale.
pub const FLOOR: u64 = 8_000_000;
pub const CEILING: u64 = 60_000_000;

static CAP: OnceLock<u64> = OnceLock::new();

/// Sets a cap in bits/s. Call at startup, before creating an encoder.
pub fn set_cap(bits: u64) {
    let _ = CAP.set(bits);
}

pub fn cap() -> Option<u64> {
    CAP.get().copied()
}

/// Bits per second for a `w`x`h` screen, honoring the cap.
///
/// 4 bits per pixel per second is what yields 8 / 15 / 33 Mbps at
/// 1080p / 1440p / 4K, and was measured as the range where the picture
/// stops smearing while scrolling a page.
pub fn target_bits(w: u32, h: u32, cap: Option<u64>) -> u32 {
    let by_area = ((w as u64) * (h as u64) * 4).clamp(FLOOR, CEILING);
    // The user's cap beats the FLOOR on purpose.
    //
    // If they said "my upload is 5 Mbps", delivering 8 because "that is
    // the minimum to look nice" is delivering a picture that never
    // arrives. The floor exists for the LAN, where the cost of erring low
    // is only quality; here the cost of erring high is the whole session.
    match cap {
        Some(t) if t > 0 => by_area.min(t) as u32,
        _ => by_area as u32,
    }
}

/// This machine's target, with the process cap already applied.
pub fn target(w: u32, h: u32) -> u32 {
    target_bits(w, h, cap())
}

/// Reads `--mbps N` from the command line. `None` = no cap.
pub fn cap_from_args(a: &[String]) -> Result<Option<u64>, String> {
    let Some(i) = a.iter().position(|v| v == "--mbps") else { return Ok(None) };
    let Some(v) = a.get(i + 1) else {
        return Err("--mbps needs a number (e.g. --mbps 6)".into());
    };
    let n: f64 = v.parse().map_err(|_| format!("--mbps: '{v}' is not a number"))?;
    if !(0.1..=1000.0).contains(&n) {
        return Err(format!("--mbps: {n} outside 0.1..1000"));
    }
    Ok(Some((n * 1_000_000.0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_with_area() {
        // The three numbers the comment always promised, and that the
        // old formula (`pixels * 60 / 1000`) did not deliver: it fell to
        // the 8 Mbps floor at any resolution.
        assert_eq!(target_bits(1920, 1080, None), 8_294_400);
        assert_eq!(target_bits(2560, 1440, None), 14_745_600);
        assert_eq!(target_bits(3840, 2160, None), 33_177_600);
    }

    #[test]
    fn mac_retina() {
        // 3360x2100 -- the screen that exposed the old bug by announcing
        // "8 Mbps" like the others.
        assert_eq!(target_bits(3360, 2100, None), 28_224_000);
    }

    #[test]
    fn cap_cuts_below_the_floor() {
        // The rule that matters most away from home: the user's cap
        // beats the floor. 5 Mbps asked for must come out as 5, not 8.
        assert_eq!(target_bits(2560, 1440, Some(5_000_000)), 5_000_000);
        assert_eq!(target_bits(1920, 1080, Some(5_000_000)), 5_000_000);
    }

    #[test]
    fn loose_cap_changes_nothing() {
        assert_eq!(target_bits(1920, 1080, Some(50_000_000)), 8_294_400);
    }

    #[test]
    fn tiny_screen_honors_the_floor_without_cap() {
        assert_eq!(target_bits(640, 480, None), FLOOR as u32);
    }

    #[test]
    fn huge_screen_honors_the_absolute_ceiling() {
        assert_eq!(target_bits(7680, 4320, None), CEILING as u32);
    }

    #[test]
    fn args_parse_and_reject() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(cap_from_args(&a(&["9777"])).unwrap(), None);
        assert_eq!(cap_from_args(&a(&["--mbps", "6"])).unwrap(), Some(6_000_000));
        assert_eq!(cap_from_args(&a(&["--mbps", "2.5"])).unwrap(), Some(2_500_000));
        assert!(cap_from_args(&a(&["--mbps"])).is_err());
        assert!(cap_from_args(&a(&["--mbps", "lots"])).is_err());
        assert!(cap_from_args(&a(&["--mbps", "0"])).is_err());
    }
}
