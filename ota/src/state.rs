// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the last check left behind, on disk.
//!
//! Spec: ../../specs/015-ota.md
//!
//! It exists because OTA runs on its own, from a scheduled task, and
//! nobody is watching when it runs. Without this file the only way to
//! know whether the machine is updating itself would be reading logs --
//! and a number nobody prints is a number nobody checks
//! ([[../../.claude/rules/verify-what-arrived]]). The wizard's `ota` step
//! reads from here.
//!
//! Only `quando_unix`, no formatted date: what the panel shows is "how
//! long ago", and formatting local time without a new dependency is the
//! kind of code that gets time zones and daylight saving wrong.
//!
//! The JSON field names (`quando_unix`, `resultado`, ...) and the
//! The `result` values are read by the wizard, so renaming one means
//! renaming it on both sides of a process boundary -- and the file on
//! disk may have been written by the build before the rename. That is
//! what `normalize_result` and `legacy_path` are for.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(alias = "quando_unix")]
    pub when_unix: u64,
    #[serde(alias = "sistema")]
    pub system: String,
    /// What is installed on this machine, as `/healthz` answers it.
    #[serde(alias = "n_aqui")]
    pub n_here: u64,
    #[serde(alias = "build_aqui")]
    pub build_here: String,
    /// What the Release published. Zeroed when we never got to know.
    #[serde(default)]
    pub tag: String,
    #[serde(default, alias = "n_la")]
    pub n_there: u64,
    #[serde(default, alias = "build_la")]
    pub build_there: String,
    #[serde(default, alias = "pacote")]
    pub package: String,
    /// `same` | `updated` | `available` | `newer-here` |
    /// `rolled-back` | `error`. A file written before the rename says
    /// `igual` and friends; `read` translates those on the way in.
    #[serde(alias = "resultado")]
    pub result: String,
    #[serde(alias = "mensagem")]
    pub message: String,
}

impl State {
    pub fn new(system: &str, n_here: u64, build_here: &str) -> State {
        State {
            when_unix: now(),
            system: system.to_string(),
            n_here,
            build_here: build_here.to_string(),
            tag: String::new(),
            n_there: 0,
            build_there: String::new(),
            package: String::new(),
            result: "error".into(),
            message: "no conclusion".into(),
        }
    }

    /// One-line summary, the way the wizard shows it.
    pub fn line(&self) -> String {
        match self.result.as_str() {
            "same" => format!("up to date on build #{} {}", self.n_here, self.build_here),
            "updated" => {
                format!("updated to #{} {} on its own", self.n_there, self.build_there)
            }
            "available" => format!(
                "build #{} {} is published; this machine has #{} -- {}",
                self.n_there, self.build_there, self.n_here, self.message
            ),
            "newer-here" => format!(
                "this machine has #{}, newer than the published #{}",
                self.n_here, self.n_there
            ),
            "rolled-back" => {
                format!("rolled back to #{} {}: {}", self.n_here, self.build_here, self.message)
            }
            _ => self.message.clone(),
        }
    }
}

/// One line: `<result> TAB <age in seconds> TAB <phrase>`.
///
/// The contract the wizard's `ota` step consumes. TAB because the phrase
/// has `;`, `:` and commas, and none of them works as a separator; TAB
/// never appears in anything `line()` produces, and the test pins that.
pub fn short(e: &State, now: u64) -> String {
    let age = now.saturating_sub(e.when_unix);
    format!("{}\t{}\t{}", e.result, age, e.line().replace(['\t', '\n'], " "))
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join("state.json")
}

/// Where an updater installed before the rename wrote its state.
///
/// Read-only and on purpose: the next write lands on the new name, so the
/// old file is picked up once and then goes quiet on its own. Deleting it
/// would be the tidier gesture and the worse one -- a rollback to the
/// previous build has to find its state where that build left it.
pub fn legacy_path(dir: &Path) -> PathBuf {
    dir.join("estado.json")
}

/// Translates the vocabulary this file used to write into the one it
/// writes now.
///
/// The state file outlives the build that wrote it: the wizard reads
/// whatever is on disk, and on an unsigned Windows box that file can sit
/// there for weeks. One translation point, on the way in, so nothing
/// downstream has to know there were ever two spellings.
pub fn normalize_result(r: &str) -> &str {
    match r {
        "igual" => "same",
        "atualizado" => "updated",
        "novidade" => "available",
        "mais-novo-aqui" => "newer-here",
        "revertido" => "rolled-back",
        "erro" => "error",
        other => other,
    }
}

pub fn write(dir: &Path, e: &State) -> Result<(), String> {
    let txt = serde_json::to_string_pretty(e).map_err(|x| x.to_string())?;
    std::fs::write(path(dir), txt).map_err(|x| format!("{}: {x}", path(dir).display()))
}

/// Reads the state. A missing or corrupt file returns `None` -- it never
/// brings down the next check.
pub fn read(dir: &Path) -> Option<State> {
    let txt = std::fs::read_to_string(path(dir))
        .or_else(|_| std::fs::read_to_string(legacy_path(dir)))
        .ok()?;
    let mut st: State = serde_json::from_str(&txt).ok()?;
    st.result = normalize_result(&st.result).to_string();
    Some(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> State {
        let mut e = State::new("mac", 124, "abc1234");
        e.tag = "v0.1.0".into();
        e.n_there = 125;
        e.build_there = "def5678".into();
        e.package = "oxdesk-mac-arm64-unsigned-125-def5678.tar.gz".into();
        e.result = "updated".into();
        e.message = "swapped and verified".into();
        e
    }

    #[test]
    fn round_trip_through_disk() {
        let dir = std::env::temp_dir().join(format!("oxdesk-ota-teste-{}", now()));
        std::fs::create_dir_all(&dir).unwrap();
        let e = sample();
        write(&dir, &e).unwrap();
        assert_eq!(read(&dir).unwrap(), e);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn garbage_in_the_file_does_not_crash() {
        let dir = std::env::temp_dir().join(format!("oxdesk-ota-scratch-{}", now()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(path(&dir), "not json").unwrap();
        assert!(read(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_file_returns_none() {
        assert!(read(Path::new("/tmp/does-not-exist-oxdesk-ota-xyz")).is_none());
    }

    /// The wire names are a contract with the wizard and with the file
    /// the previous version wrote: renaming a Rust field must not move
    /// them.
    #[test]
    fn json_keys_keep_their_wire_names() {
        // This test existed to stop the wire names being renamed by
        // accident, and it did: renaming them broke it first. Now it pins
        // the other half of a deliberate rename -- what we write, and
        // what we must still be able to read.
        let txt = serde_json::to_string(&sample()).unwrap();
        for k in [
            "when_unix", "system", "n_here", "build_here", "tag", "n_there", "build_there",
            "package", "result", "message",
        ] {
            assert!(txt.contains(&format!("\"{k}\":")), "{k} missing in {txt}");
        }
        for gone in ["quando_unix", "n_aqui", "resultado", "mensagem"] {
            assert!(!txt.contains(&format!("\"{gone}\":")), "{gone} should no longer be written");
        }
    }

    /// Every word the old vocabulary could hold maps to one of the new
    /// ones, and a word we do not know passes through rather than
    /// becoming an empty string the wizard cannot report.
    #[test]
    fn every_old_word_has_a_new_one() {
        for (old, new) in [
            ("igual", "same"),
            ("atualizado", "updated"),
            ("novidade", "available"),
            ("mais-novo-aqui", "newer-here"),
            ("revertido", "rolled-back"),
            ("erro", "error"),
        ] {
            assert_eq!(normalize_result(old), new);
            assert_eq!(normalize_result(new), new, "the new word must survive a second pass");
        }
        assert_eq!(normalize_result("banana"), "banana");
    }

    /// Every result has to produce a phrase that says what happened.
    /// An empty line in the panel is the same as having no panel.
    #[test]
    fn every_result_has_a_phrase() {
        for r in ["same", "updated", "available", "newer-here", "rolled-back", "error"] {
            let mut e = sample();
            e.result = r.into();
            let l = e.line();
            assert!(!l.trim().is_empty(), "{r} says nothing");
            assert!(l.len() > 8, "{r}: \"{l}\"");
        }
    }

    /// Three fields, always, and TAB only between them -- that is what
    /// the wizard splits on. A phrase with a TAB inside would break the
    /// panel's `ota` step without breaking any test here, so the test is
    /// here.
    #[test]
    fn short_has_three_fields_and_no_tab_inside() {
        for r in ["same", "updated", "available", "newer-here", "rolled-back", "error"] {
            let mut e = sample();
            e.result = r.into();
            e.when_unix = 1000;
            let l = short(&e, 1725);
            let fields: Vec<&str> = l.split('\t').collect();
            assert_eq!(fields.len(), 3, "{r}: {l:?}");
            assert_eq!(fields[0], r);
            assert_eq!(fields[1], "725", "the age is now minus when");
            assert!(!fields[2].trim().is_empty());
        }
    }

    /// A clock that went backwards (or a state from the future) must not
    /// turn into a huge age by overflow.
    #[test]
    fn age_is_never_negative_and_never_overflows() {
        let mut e = sample();
        e.when_unix = 9999;
        assert!(short(&e, 10).contains("\t0\t"));
    }

    /// A field added one day must not invalidate the file the previous
    /// version wrote -- otherwise the first check after a deploy loses
    /// the history.
    #[test]
    fn old_state_without_new_fields_still_reads() {
        let dir = std::env::temp_dir().join(format!("oxdesk-ota-old-{}", now()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            path(&dir),
            r#"{"quando_unix":1,"sistema":"linux","n_aqui":1,"build_aqui":"a","resultado":"igual","mensagem":"x"}"#,
        )
        .unwrap();
        let e = read(&dir).expect("the defaulted fields have to cover it");
        assert_eq!(e.n_there, 0);
        // The field names AND the word: both were renamed, and a file on
        // disk knows nothing about it.
        assert_eq!(e.n_here, 1, "`n_aqui` has to still land in `n_here`");
        assert_eq!(e.result, "same", "`igual` has to arrive as `same`");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The file an updater installed before the rename actually wrote, at
    /// the name it wrote it under. Without this the wizard would report
    /// "the timer is not firing" on a machine whose timer fired fine --
    /// the state was simply being looked for under the new name.
    #[test]
    fn a_state_file_under_the_old_name_is_still_found() {
        let dir = std::env::temp_dir().join(format!("oxdesk-ota-legacy-{}", now()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            legacy_path(&dir),
            r#"{"quando_unix":7,"sistema":"windows","n_aqui":9,"build_aqui":"b","resultado":"novidade","mensagem":"y"}"#,
        )
        .unwrap();
        let e = read(&dir).expect("estado.json has to be read when state.json is absent");
        assert_eq!((e.n_here, e.result.as_str()), (9, "available"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// And the new name wins, so the fallback cannot resurrect a stale
    /// answer once this build has written one.
    #[test]
    fn the_new_name_wins_over_the_old_one() {
        let dir = std::env::temp_dir().join(format!("oxdesk-ota-both-{}", now()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(legacy_path(&dir), r#"{"quando_unix":1,"sistema":"l","n_aqui":1,"build_aqui":"old","resultado":"igual","mensagem":"x"}"#).unwrap();
        let mut fresh = State::new("linux", 2, "new");
        fresh.result = "same".into();
        write(&dir, &fresh).unwrap();
        let e = read(&dir).expect("state.json exists");
        assert_eq!(e.build_here, "new", "the fallback must not shadow a fresh write");
        std::fs::remove_dir_all(&dir).ok();
    }
}
