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
fn classify(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("falhou") || m.contains("error") || m.contains("fatal") || m.contains("nao trocou")
    {
        "error"
    } else if m.contains("aviso")
        || m.contains("estado dividido")
        || m.contains("cooldown")
        || m.contains("desarmada")
    {
        "warn"
    } else {
        "info"
    }
}

/// Lines that only add noise: the D-Bus signal echo and the heartbeat.
fn noise(msg: &str) -> bool {
    msg.starts_with("border[sinal]") || msg.contains("border: vivo")
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
        assert_eq!(classify("slot 1 FALHOU: sem resposta"), "error");
        assert_eq!(classify("border: cooldown ate 18:00"), "warn");
        assert_eq!(classify("troca concluida em 46ms"), "info");
    }

    #[test]
    fn noise_is_dropped() {

        assert!(noise("border[sinal] Activated x=2560.0"));
        assert!(noise("border: vivo"));
        assert!(!noise("border: permanencia 1/1, trocando"));
    }
}
