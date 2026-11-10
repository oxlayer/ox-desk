// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Which address the host listens on.
//!
//! Spec: ../../specs/012-outside-the-lan.md
//!
//! ## Why this exists
//!
//! The host always listened on `0.0.0.0`, and at home that was inside
//! spec 008's trust model: "everything on the LAN". But the screen
//! channel **encrypts and does not authenticate** -- the client accepts
//! any certificate (`AceitaQualquer` in `net.rs`, marked as temporary
//! since it was written; today `identity::PinnedHost` without a pin).
//! Whoever reaches 9777 watches the screen, and whoever reaches 9778
//! types on it.
//!
//! While the three machines stayed at home, the reach was the home LAN.
//! Taking the MacBook out changes that: on a hotel Wi-Fi, `0.0.0.0`
//! serves the screen and keyboard to the whole hotel network.
//!
//! ## The rule that drives the design
//!
//! `overlay` **fails** when it cannot find the overlay address. It does
//! not fall back to `0.0.0.0`.
//!
//! The silent fallback would be comfortable and would be exactly the bug:
//! the person asked for "overlay only", Tailscale did not come up, and
//! the host would open to the hotel Wi-Fi without anyone knowing. Same
//! family as the `backfill_gate` that did not gate and the `catch` that
//! swallowed `MODULE_NOT_FOUND`. Loud error > imaginary protection.
//!
//! ## Why looking at 100.64.0.0/10 is not enough
//!
//! That range is CGNAT (RFC 6598), and Tailscale uses it on purpose.
//! But it is real CGNAT too: a phone sharing its connection hands out
//! addresses in that range. The MacBook on a hotspot -- which is exactly
//! the test case -- would have an `en0` on 100.x and the host would take
//! it for the overlay, listening on the phone's network.
//!
//! That is why the criterion is **range + interface name**.

use std::net::{IpAddr, Ipv4Addr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// `0.0.0.0` -- the usual behavior. At home, on the LAN.
    All,
    /// Only the overlay address. For the machine that leaves home.
    Overlay,
    /// A hand-picked address.
    Fixed(IpAddr),
}

impl Mode {
    pub fn parse(s: &str) -> Result<Mode, String> {
        match s {
            "todas" | "all" | "0.0.0.0" => Ok(Mode::All),
            "overlay" | "tailscale" => Ok(Mode::Overlay),
            other => other
                .parse::<IpAddr>()
                .map(Mode::Fixed)
                .map_err(|_| format!("--bind: '{other}' is not 'todas', 'overlay' or an address")),
        }
    }
}

/// The overlay address, among this machine's interfaces.
///
/// `interfaces` is `(name, address)`. Pure so it is testable: the real
/// list comes from `local_addrs()`, and the cases that matter -- hotspot
/// with CGNAT, Tailscale down, two candidate interfaces -- need no
/// machine at all to be exercised.
pub fn find_overlay(interfaces: &[(String, IpAddr)]) -> Result<IpAddr, String> {
    let candidates: Vec<&(String, IpAddr)> =
        interfaces.iter().filter(|(name, ip)| is_overlay(name, ip)).collect();
    if candidates.is_empty() {
        return Err(
            "no overlay address found on this machine. Is Tailscale connected? \
             (`tailscale status`). NOT listening on 0.0.0.0 instead."
                .into(),
        );
    }

    // IPv4 first, and this is NOT an aesthetic preference.
    //
    // The first version required a single candidate and failed on every
    // real machine: Tailscale gives v4 **and** v6 on the same interface,
    // and "found 2 overlay addresses" reached me from the real Windows
    // box (Tailscale=fd7a:115c:a1e0::ab12:cd34 and Tailscale=100.88.10.20).
    // In other words, `--bind overlay` would never have worked anywhere
    // -- and the tests did not catch it because I wrote both cases with
    // two INTERFACES, which is the rare case, instead of one interface
    // with both families, which is the normal case.
    //
    // v4 because a UDP socket bound to a v6 address does not receive v4
    // traffic, and peers will speak v4 by default.
    let v4: Vec<&&(String, IpAddr)> = candidates.iter().filter(|(_, ip)| ip.is_ipv4()).collect();
    match v4.len() {
        1 => return Ok(v4[0].1),
        // Two overlay v4s is a state that should not exist, and choosing
        // silently would choose wrong half the time.
        n if n > 1 => {
            return Err(format!(
                "found {n} overlay IPv4 addresses ({}); use --bind <address> to decide",
                list(&v4.iter().map(|x| **x).collect::<Vec<_>>())
            ))
        }
        _ => {}
    }

    // IPv6 only: a tailnet without v4 is possible. One candidate will do;
    // several, there is no guessing.
    match candidates.len() {
        1 => Ok(candidates[0].1),
        n => Err(format!(
            "found {n} overlay IPv6 addresses ({}); use --bind <address> to decide",
            list(&candidates)
        )),
    }
}

fn list(v: &[&(String, IpAddr)]) -> String {
    v.iter().map(|(n, ip)| format!("{n}={ip}")).collect::<Vec<_>>().join(", ")
}

/// Is this interface the overlay?
///
/// Range **and** name. The names, per system:
///
/// | OS | interface |
/// |---|---|
/// | Linux | `tailscale0` |
/// | macOS | `utun3`, `utun4`... (the index changes every boot) |
/// | Windows | `Tailscale` (the adapter's friendly name) |
///
/// On macOS the name alone is not enough: corporate VPNs, iCloud Private
/// Relay and the system itself create `utun`. It is the range+name pair
/// that identifies it.
fn is_overlay(name: &str, ip: &IpAddr) -> bool {
    if !in_tailscale_range(ip) {
        return false;
    }
    let n = name.to_ascii_lowercase();
    n.starts_with("tailscale") || n.starts_with("utun") || n.contains("tailscale")
}

/// 100.64.0.0/10 (IPv4) or fd7a:115c:a1e0::/48 (IPv6).
fn in_tailscale_range(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (64..=127).contains(&o[1])
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    }
}

/// The final address, already resolved.
pub fn resolve(mode: &Mode, interfaces: &[(String, IpAddr)]) -> Result<IpAddr, String> {
    match mode {
        Mode::All => Ok(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        Mode::Fixed(ip) => Ok(*ip),
        Mode::Overlay => find_overlay(interfaces),
    }
}

/// This machine's interfaces, for real.
pub fn local_addrs() -> Vec<(String, IpAddr)> {
    match if_addrs::get_if_addrs() {
        Ok(v) => v.into_iter().map(|i| (i.name, i.addr.ip())).collect(),
        Err(e) => {
            eprintln!("  could not list interfaces: {e}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i(name: &str, ip: &str) -> (String, IpAddr) {
        (name.into(), ip.parse().unwrap())
    }

    #[test]
    fn finds_tailscale_on_linux() {
        let ifs = [i("lo", "127.0.0.1"), i("eno1", "192.168.1.50"), i("tailscale0", "100.101.102.103")];
        assert_eq!(find_overlay(&ifs).unwrap(), "100.101.102.103".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn finds_tailscale_on_mac() {
        // The utun index changes every boot; matching a fixed "utun3"
        // would work today and fail tomorrow.
        let ifs = [i("en0", "192.168.1.60"), i("utun7", "100.72.1.9")];
        assert_eq!(find_overlay(&ifs).unwrap(), "100.72.1.9".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn hotspot_with_cgnat_is_not_overlay() {
        // THIS is the case the range alone would get wrong, and it is
        // exactly tomorrow's test scenario: MacBook tethered to the phone.
        // `en0` gets a 100.x address from the carrier, and listening on
        // it would serve the screen to the phone's network.
        let ifs = [i("en0", "100.90.1.5"), i("lo0", "127.0.0.1")];
        assert!(find_overlay(&ifs).is_err());
    }

    #[test]
    fn no_overlay_fails_and_does_not_fall_back_to_all() {
        // This module's whole rule in one test: `resolve` with Overlay
        // and no overlay must FAIL. If someday someone "improves" this
        // with a fallback, the test breaks -- and it is meant to.
        let ifs = [i("en0", "192.168.1.60")];
        let e = resolve(&Mode::Overlay, &ifs).unwrap_err();
        assert!(e.contains("no overlay address"), "{e}");
        assert!(!e.contains("0.0.0.0") || e.contains("NOT listening"), "{e}");
    }

    #[test]
    fn v4_and_v6_on_the_same_interface_is_the_NORMAL_case() {
        // The case the first version treated as an error, and that came
        // from the real Windows box: Tailscale gives both families on the
        // same interface. Requiring "a single candidate" made
        // `--bind overlay` fail on EVERY machine.
        let ifs = [
            i("Tailscale", "fd7a:115c:a1e0::ab12:cd34"),
            i("Tailscale", "100.88.10.20"),
            i("Ethernet", "192.168.1.42"),
        ];
        assert_eq!(find_overlay(&ifs).unwrap(), "100.88.10.20".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn ipv6_only_still_works() {
        let ifs = [i("tailscale0", "fd7a:115c:a1e0::1234")];
        assert_eq!(find_overlay(&ifs).unwrap(), "fd7a:115c:a1e0::1234".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn two_overlay_v4s_ask_for_a_decision() {
        let ifs = [i("tailscale0", "100.1.1.1"), i("utun5", "100.2.2.2")];
        assert!(find_overlay(&ifs).is_err());
    }

    #[test]
    fn neighboring_range_does_not_count() {
        // 100.63 and 100.128 are OUTSIDE the /10. Getting the mask math
        // wrong here would open the machine on a public range.
        assert!(!in_tailscale_range(&"100.63.0.1".parse().unwrap()));
        assert!(!in_tailscale_range(&"100.128.0.1".parse().unwrap()));
        assert!(in_tailscale_range(&"100.64.0.1".parse().unwrap()));
        assert!(in_tailscale_range(&"100.127.255.254".parse().unwrap()));
    }

    #[test]
    fn mode_parses_the_three_forms() {
        assert_eq!(Mode::parse("todas").unwrap(), Mode::All);
        assert_eq!(Mode::parse("overlay").unwrap(), Mode::Overlay);
        assert_eq!(Mode::parse("10.0.0.5").unwrap(), Mode::Fixed("10.0.0.5".parse().unwrap()));
        assert!(Mode::parse("sim").is_err());
    }

    #[test]
    fn all_is_still_todays_default() {
        assert_eq!(resolve(&Mode::All, &[]).unwrap(), "0.0.0.0".parse::<IpAddr>().unwrap());
    }
}

// ------------------------------------------------- the process's choice
//
// Resolved ONCE, at startup, and read by the three hosts. The alternative
// -- threading an `IpAddr` through three platform signatures that today
// take `(port, screen)` -- would spread the parameter through code that
// has nothing to do with networking. Here it is process configuration,
// decided before any socket exists.

use std::sync::OnceLock;

static LISTEN: OnceLock<IpAddr> = OnceLock::new();

/// Sets the listen address. Call before opening any socket.
pub fn set_listen_addr(ip: IpAddr) {
    let _ = LISTEN.set(ip);
}

/// The listen address. `0.0.0.0` when nobody chose -- which is the
/// behavior the host always had.
pub fn listen_addr() -> IpAddr {
    *LISTEN.get().unwrap_or(&IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}
