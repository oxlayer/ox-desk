// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! One event per screen session, written locally when it ends.
//!
//! Spec: ../specs/017-product.md (section 2)
//!
//! It exists to answer one product question with numbers: how many sessions
//! go direct and how many fall back to a relay, at home, on 4G and away. So
//! it carries the path and how the session went, and nothing that says who,
//! where or what: no IP, no machine or user name, no file path, no screen,
//! keyboard or clipboard content, no identity fingerprint.
//!
//! Until our rendezvous server exists, the event only goes to a local file
//! (`telemetry.jsonl` in the state dir). Who writes it decides whether it is
//! enabled: `oxdesk` passes the file path only when telemetry is on.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// Why the session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The viewer closed it.
    Closed,
    /// The connection to the host failed mid-session.
    Dropped,
    /// It never connected: refused (pairing, identity) or unreachable.
    Refused,
}

impl End {
    fn as_str(self) -> &'static str {
        match self {
            End::Closed => "closed",
            End::Dropped => "dropped",
            End::Refused => "refused",
        }
    }
}

/// What one session measured. Filled by the bridge as it runs.
#[derive(Debug, Default)]
pub struct Session {
    /// Build stamp of the panel that opened it.
    pub version: String,
    pub host_os: String,
    pub client_os: String,
    /// `lan`, `overlay-direct`, `overlay-relay`, `internet` or `unknown`,
    /// decided by `oxdesk` when the session starts.
    pub path: String,
    pub frames: u64,
    pub bytes: u64,
    pub frames_lost: u64,
    pub keyframe_requests: u64,
    rtt_ms: Vec<f64>,
}

impl Session {
    pub fn new(version: String, host_os: String, client_os: String, path: String) -> Self {
        Self { version, host_os, client_os, path, ..Default::default() }
    }

    pub fn rtt_sample(&mut self, rtt: Duration) {
        // A sample a second is plenty; this caps memory for day-long sessions.
        if self.rtt_ms.len() < 100_000 {
            self.rtt_ms.push(rtt.as_secs_f64() * 1000.0);
        }
    }

    /// The JSON line for this session. Hand-written because every value is
    /// a number or one of our own short strings; `esc` covers the rest.
    pub fn to_json(&self, end: End, duration: Duration) -> String {
        let secs = duration.as_secs_f64().max(0.001);
        let (p50, p95) = percentiles(&self.rtt_ms);
        format!(
            concat!(
                "{{\"v\":1,\"version\":\"{}\",\"host_os\":\"{}\",\"client_os\":\"{}\",\"path\":\"{}\",",
                "\"end\":\"{}\",\"minutes\":{},\"rtt_ms_p50\":{},\"rtt_ms_p95\":{},",
                "\"fps\":{:.1},\"mbps\":{:.2},\"frames_lost\":{},\"keyframe_requests\":{}}}"
            ),
            esc(&self.version),
            esc(&self.host_os),
            esc(&self.client_os),
            esc(&self.path),
            end.as_str(),
            // Rounded minutes, as the spec says: the length of a session is
            // useful, its exact second is not.
            (secs / 60.0).round() as u64,
            opt(p50),
            opt(p95),
            self.frames as f64 / secs,
            self.bytes as f64 * 8.0 / secs / 1_000_000.0,
            self.frames_lost,
            self.keyframe_requests,
        )
    }

    /// Appends the event to `file`. Failing to write telemetry never
    /// affects the session: the error goes to stderr and that is it.
    pub fn append(&self, file: &Path, end: End, duration: Duration) {
        let line = self.to_json(end, duration);
        let r = std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))
            .and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(file))
            .and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = r {
            eprintln!("telemetry: could not write {}: {e}", file.display());
        }
    }
}

fn opt(v: Option<f64>) -> String {
    v.map_or_else(|| "null".into(), |x| format!("{x:.1}"))
}

fn percentiles(v: &[f64]) -> (Option<f64>, Option<f64>) {
    if v.is_empty() {
        return (None, None);
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |q: f64| s[((s.len() - 1) as f64 * q).round() as usize];
    (Some(at(0.5)), Some(at(0.95)))
}

/// JSON string escaping for the few free-form values (version, OS names).
fn esc(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            c if (c as u32) < 0x20 => vec![],
            c => vec![c],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            version: "#209 b6e14dd".into(),
            host_os: "linux".into(),
            client_os: "macos".into(),
            path: "overlay-direct".into(),
            frames: 600,
            bytes: 10_000_000,
            frames_lost: 2,
            keyframe_requests: 1,
            ..Default::default()
        }
    }

    #[test]
    fn one_line_with_the_numbers_and_nothing_else() {
        let mut s = session();
        for ms in [10, 20, 30, 40, 200] {
            s.rtt_sample(Duration::from_millis(ms));
        }
        let j = s.to_json(End::Closed, Duration::from_secs(10));
        assert!(!j.contains('\n'));
        assert!(j.contains("\"path\":\"overlay-direct\""), "{j}");
        assert!(j.contains("\"end\":\"closed\""), "{j}");
        assert!(j.contains("\"fps\":60.0"), "{j}");
        assert!(j.contains("\"mbps\":8.00"), "{j}");
        assert!(j.contains("\"rtt_ms_p50\":30.0"), "{j}");
        assert!(j.contains("\"rtt_ms_p95\":200.0"), "{j}");
        assert!(j.contains("\"minutes\":0"), "{j}");
    }

    #[test]
    fn no_rtt_is_null_not_zero() {
        let j = session().to_json(End::Refused, Duration::ZERO);
        assert!(j.contains("\"rtt_ms_p50\":null"), "{j}");
        assert!(j.contains("\"end\":\"refused\""), "{j}");
    }

    #[test]
    fn free_text_cannot_break_the_line() {
        let mut s = session();
        s.version = "a\"b\\c\nd".into();
        let j = s.to_json(End::Dropped, Duration::from_secs(1));
        assert!(j.contains("\"version\":\"a\\\"b\\\\cd\""), "{j}");
    }

    #[test]
    fn appends_one_line_per_session() {
        let d = std::env::temp_dir().join(format!("oxdesk-telemetry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let f = d.join("telemetry.jsonl");
        session().append(&f, End::Closed, Duration::from_secs(5));
        session().append(&f, End::Dropped, Duration::from_secs(5));
        let text = std::fs::read_to_string(&f).unwrap();
        assert_eq!(text.lines().count(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }
}
