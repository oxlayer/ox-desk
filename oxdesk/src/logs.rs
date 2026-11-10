// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Log for the GUI: the policy lives here, the source in the platform.
//!
//! It exists because its absence cost a wrong diagnosis: the channel
//! switch took 105 s and the user, seeing nothing, concluded it failed.
//! The panel has to show what the daemon is doing.
//!
//! The split is deliberate. `sys::log_lines` hands over (time, message)
//! pairs -- journal on Linux, a file on Windows -- and the classification
//! and the noise filter live here, once. Two copies of the heuristic
//! would diverge, and the first time they did I would not notice.

use serde::Serialize;

const DEFAULT_LINES: usize = 200;
const MAX_LINES: usize = 1000;

/// What the platform hands over: already in order, already without a
/// header.
pub struct Raw {
    /// Where it came from -- systemd unit or file path.
    pub unit: String,
    pub lines: Vec<(String, String)>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub time: String,
    pub message: String,
    /// info | warn | error -- a heuristic on the text, because the daemon
    /// writes to stdout/stderr and does not use syslog priorities.
    pub level: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Logs {
    pub unit: String,
    pub entries: Vec<Entry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The patterns match the log lines written by `border`, `follow` and
/// `focus`; whoever rewords those lines updates these too.
///
/// It used to also match `falhou`, because `winput` wrote its errors in
/// pt-BR and they reach the panel through these modules. `winput` speaks
/// English since specs/018 step 6f, so the word went with it -- lines
/// already in the rotated log that contain it now classify as info, which
/// is cosmetic and only touches history.
fn classify(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("failed")
        || m.contains("error")
        || m.contains("fatal")
        || m.contains("did not switch")
    {
        "error"
    } else if m.contains("warning")
        || m.contains("split state")
        || m.contains("cooldown")
        || m.contains("disarmed")
    {
        "warn"
    } else {
        "info"
    }
}

/// Lines that only add noise: the D-Bus signal echo and the heartbeat.
fn noise(msg: &str) -> bool {
    msg.starts_with("border[signal]") || msg.contains("border: alive")
}

pub fn tail(lines: usize) -> Logs {
    let raw = crate::sys::log_lines(lines.clamp(1, MAX_LINES));
    let entries = raw
        .lines
        .into_iter()
        .filter(|(_, msg)| !noise(msg))
        .map(|(time, message)| Entry { time, level: classify(&message), message })
        .collect();
    Logs { unit: raw.unit, entries, error: raw.error }
}

pub fn default_lines() -> usize {
    DEFAULT_LINES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_text() {
        assert_eq!(classify("border: MX Keys: FAILED -- slot 1 does not answer"), "error");
        assert_eq!(classify("border: did NOT switch -- plan not ready (blockers: hidraw)"), "error");
        assert_eq!(classify("border: SPLIT STATE: 1 switched, but 1 stayed here"), "warn");
        assert_eq!(classify("border: disarmed -- the devices are not here"), "warn");
        assert_eq!(classify("border: cooldown over, watching again"), "warn");
        assert_eq!(classify("border: armed -- the devices are here"), "info");
        assert_eq!(classify("border: input detected -- dead-man's switch stood down"), "info");
        assert_eq!(classify("switch done in 46ms"), "info");
        // An error text coming up from `winput` still counts.
        assert_eq!(classify("clip: GlobalLock failed on read"), "error");
    }

    #[test]
    fn noise_is_dropped() {
        assert!(noise("border[signal] Activated x=2560.0"));
        assert!(noise("border: alive, 3 min watching 1 edge(s), armed"));
        assert!(!noise("border: dwell 1/2, waiting"));
    }
}
