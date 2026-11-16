// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Who may talk to the local API: this machine's panel, tray and CLI, and
//! no web page.
//!
//! Spec: ../../specs/008-security.md
//!
//! The API listens on `127.0.0.1` only, and that was taken to mean "only
//! this machine". It does not mean "only us": every web page open in a
//! browser on this machine can send requests to 127.0.0.1. Measured on
//! 2026-11-15, on the reference Windows machine:
//!
//! - a `POST` with `Origin: https://evil.example` and a `text/plain` body
//!   -- the kind a browser sends from any site without asking first --
//!   answered 200 and acted;
//! - a `GET` with `Host: evil.example:8787` answered 200, which is what a
//!   DNS-rebinding page needs to READ the answers too.
//!
//! The routes behind it send the keyboard and mouse to another machine,
//! kill processes, write clipboards and change the config. So two rules,
//! checked before any route runs:
//!
//! 1. **The `Host` is a loopback name.** A rebinding page points its own
//!    domain at 127.0.0.1, and its requests carry that domain as `Host`.
//! 2. **A command (anything but `GET`/`HEAD`) from a browser comes from
//!    this daemon's own page.** A browser always says where a cross-site
//!    request comes from (`Origin`), and our own panel's origin is
//!    `http://127.0.0.1:<port>` (or `localhost`). The tray (Electron's
//!    main process) and the CLI send no `Origin` at all, and pass.
//!
//! Found by reading another KVM's code, which keeps "web pages reaching
//! localhost" out of its control channel with a token.

/// Why a request was refused, or `Ok` to let it through.
pub fn check(
    method: &str,
    host: Option<&str>,
    origin: Option<&str>,
    port: u16,
) -> Result<(), &'static str> {
    let Some(host) = host else {
        return Err("no Host header");
    };
    if !loopback_authority(host, None) {
        return Err("the Host is not this machine");
    }
    if matches!(method, "GET" | "HEAD") {
        return Ok(());
    }
    match origin {
        None => Ok(()),
        Some(o) => {
            let Some(authority) = o.strip_prefix("http://") else {
                // `null` (a sandboxed frame, a file), `https://...` -- none
                // of them is our page, which is served over plain http.
                return Err("a command from a page that is not this panel");
            };
            if loopback_authority(authority, Some(port)) {
                Ok(())
            } else {
                Err("a command from a page that is not this panel")
            }
        }
    }
}

/// Is `authority` (`host[:port]`) a loopback name -- and, when `port` is
/// given, on exactly that port?
fn loopback_authority(authority: &str, port: Option<u16>) -> bool {
    let (name, given) = match authority.strip_prefix('[') {
        // IPv6 literal: `[::1]` or `[::1]:8787`.
        Some(rest) => match rest.split_once(']') {
            Some((name, tail)) => (name, tail.strip_prefix(':')),
            None => return false,
        },
        None => match authority.rsplit_once(':') {
            Some((name, p)) => (name, Some(p)),
            None => (authority, None),
        },
    };
    let name_ok = matches!(name.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1");
    let port_ok = match (port, given) {
        (None, _) => true,
        (Some(want), Some(p)) => p.parse::<u16>() == Ok(want),
        // An origin always carries the port unless it is the scheme's
        // default (80 for http), which is never ours.
        (Some(_), None) => false,
    };
    name_ok && port_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: u16 = 8787;

    #[test]
    fn the_panel_the_tray_and_the_cli_get_through() {
        // The panel in the GUI or a browser: same origin.
        assert_eq!(check("POST", Some("127.0.0.1:8787"), Some("http://127.0.0.1:8787"), P), Ok(()));
        assert_eq!(check("POST", Some("localhost:8787"), Some("http://localhost:8787"), P), Ok(()));
        assert_eq!(check("PATCH", Some("[::1]:8787"), Some("http://[::1]:8787"), P), Ok(()));
        // The tray (Node's http) and the CLI (a raw request): no Origin.
        assert_eq!(check("POST", Some("127.0.0.1:8787"), None, P), Ok(()));
        assert_eq!(check("POST", Some("127.0.0.1"), None, P), Ok(()));
        // Reads from anywhere on this machine.
        assert_eq!(check("GET", Some("localhost"), None, P), Ok(()));
    }

    #[test]
    fn a_web_page_cannot_send_a_command() {
        // The probe that answered 200 before this existed.
        assert!(check("POST", Some("127.0.0.1:8787"), Some("https://evil.example"), P).is_err());
        assert!(check("POST", Some("127.0.0.1:8787"), Some("null"), P).is_err());
        // Another local app's page, on another port.
        assert!(check("POST", Some("127.0.0.1:8787"), Some("http://127.0.0.1:3000"), P).is_err());
        // A look-alike that only starts with a loopback name.
        assert!(check("POST", Some("127.0.0.1:8787"), Some("http://127.0.0.1.evil.example:8787"), P).is_err());
        assert!(check("POST", Some("127.0.0.1:8787"), Some("http://localhost"), P).is_err());
    }

    #[test]
    fn a_rebinding_page_cannot_even_read() {
        assert!(check("GET", Some("evil.example:8787"), None, P).is_err());
        assert!(check("GET", Some("127.0.0.1.evil.example"), None, P).is_err());
        assert!(check("GET", None, None, P).is_err());
    }
}
