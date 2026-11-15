// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "Check for updates", asked from the tray.
//!
//! The updater, `oxdesk-ota`, owns the question: `--check` compares this
//! machine's build with the newest published Release and records what it
//! found, without swapping anything. This module runs it, reads the record
//! and says it in one sentence. Installing stays where it was: the
//! updater's own hourly timer on Linux and macOS, and the deploy on
//! Windows, which is what signs the binaries there ([[specs/015-ota]]).

use serde::{Deserialize, Serialize};

/// What the tray shows.
#[derive(Debug, Serialize, PartialEq)]
pub struct Outcome {
    /// `same`, `available`, `newer-here` or `error`, as the updater says.
    pub result: String,
    pub message: String,
}

/// The fields of `oxdesk-ota --state` this needs.
#[derive(Debug, Default, Deserialize)]
struct Record {
    #[serde(default)]
    n_here: u64,
    #[serde(default)]
    n_there: u64,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    result: String,
    #[serde(default)]
    message: String,
}

/// Runs the check and reads its record. Can take seconds: call it off the
/// UI thread.
pub fn check() -> Outcome {
    let bin = crate::sys::own_bin("oxdesk-ota");
    let bin = bin.to_string_lossy().into_owned();
    // A failed check exits non-zero, and its record still says why: the
    // record is read either way.
    let _ = crate::setup_common::run_timed(&bin, &["--check"], 60);
    match crate::setup_common::run_timed(&bin, &["--state"], 8) {
        Ok(json) => verdict(&json, cfg!(windows)),
        Err(e) => Outcome { result: "error".into(), message: format!("Could not ask the updater: {e}") },
    }
}

/// The sentence, from the updater's record. Pure, so each case is tested.
pub fn verdict(json: &str, windows: bool) -> Outcome {
    let r: Record = match serde_json::from_str(json) {
        Ok(r) => r,
        Err(e) => {
            return Outcome { result: "error".into(), message: format!("Could not read the updater's state: {e}") }
        }
    };
    let message = match r.result.as_str() {
        "same" => format!("OxDesk is up to date: this machine runs the newest release, #{}.", r.n_here),
        "available" if windows => format!(
            "OxDesk #{} ({}) is available; this machine runs #{}. On Windows it is installed by the deploy.",
            r.n_there, r.tag, r.n_here
        ),
        "available" => format!(
            "OxDesk #{} ({}) is available; this machine runs #{}. It installs at the next hourly check.",
            r.n_there, r.tag, r.n_here
        ),
        "newer-here" => format!(
            "This machine runs #{}, newer than the newest release (#{}).",
            r.n_here, r.n_there
        ),
        _ if r.message.is_empty() => "Could not check for updates.".to_string(),
        _ => format!("Could not check for updates: {}", r.message),
    };
    let result = if r.result.is_empty() { "error".into() } else { r.result };
    Outcome { result, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AVAILABLE: &str = r#"{"n_here":282,"n_there":300,"tag":"v0.3.0","result":"available","message":"only checked (--check)"}"#;

    #[test]
    fn up_to_date_names_the_build() {
        let o = verdict(r#"{"n_here":300,"n_there":300,"tag":"v0.3.0","result":"same","message":"x"}"#, false);
        assert_eq!(o.result, "same");
        assert!(o.message.contains("up to date") && o.message.contains("#300"), "{}", o.message);
    }

    #[test]
    fn available_says_how_it_installs_on_each_system() {
        let linux = verdict(AVAILABLE, false);
        assert_eq!(linux.result, "available");
        assert!(linux.message.contains("#300 (v0.3.0)") && linux.message.contains("#282"), "{}", linux.message);
        assert!(linux.message.contains("hourly"), "{}", linux.message);
        let windows = verdict(AVAILABLE, true);
        assert!(windows.message.contains("deploy"), "{}", windows.message);
        assert!(!windows.message.contains("hourly"), "{}", windows.message);
    }

    #[test]
    fn newer_here_is_not_an_update() {
        let o = verdict(r#"{"n_here":310,"n_there":300,"result":"newer-here"}"#, false);
        assert!(o.message.contains("#310") && o.message.contains("newer"), "{}", o.message);
    }

    #[test]
    fn an_error_carries_the_updaters_reason() {
        let o = verdict(r#"{"result":"error","message":"GitHub answered: Not Found"}"#, false);
        assert_eq!(o.result, "error");
        assert!(o.message.ends_with("GitHub answered: Not Found"), "{}", o.message);
    }

    #[test]
    fn an_unreadable_record_is_an_error_not_a_panic() {
        assert_eq!(verdict("not json", false).result, "error");
        assert_eq!(verdict("{}", false).result, "error");
    }
}
