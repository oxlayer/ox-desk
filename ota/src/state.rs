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
//! `resultado` values are read by the wizard and by
//! `scripts/distribute-ota-token.sh`; the phrases from `line()` are shown
//! in the panel, so they stay in Portuguese.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(rename = "quando_unix")]
    pub when_unix: u64,
    #[serde(rename = "sistema")]
    pub system: String,
    /// What is installed on this machine, as `/healthz` answers it.
    #[serde(rename = "n_aqui")]
    pub n_here: u64,
    #[serde(rename = "build_aqui")]
    pub build_here: String,
    /// What the Release published. Zeroed when we never got to know.
    #[serde(default)]
    pub tag: String,
    #[serde(default, rename = "n_la")]
    pub n_there: u64,
    #[serde(default, rename = "build_la")]
    pub build_there: String,
    #[serde(default, rename = "pacote")]
    pub package: String,
    /// `igual` | `atualizado` | `novidade` | `mais-novo-aqui` |
    /// `revertido` | `erro`
    #[serde(rename = "resultado")]
    pub result: String,
    #[serde(rename = "mensagem")]
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
            result: "erro".into(),
            message: "sem conclusao".into(),
        }
    }

    /// One-line summary, the way the wizard shows it.
    pub fn line(&self) -> String {
        match self.result.as_str() {
            "igual" => format!("em dia no build #{} {}", self.n_here, self.build_here),
            "atualizado" => {
                format!("atualizei para #{} {} sozinho", self.n_there, self.build_there)
            }
            "novidade" => format!(
                "ha o build #{} {} publicado; aqui esta o #{} -- {}",
                self.n_there, self.build_there, self.n_here, self.message
            ),
            "mais-novo-aqui" => format!(
                "aqui esta o #{}, mais novo que o #{} publicado",
                self.n_here, self.n_there
            ),
            "revertido" => format!("voltei para #{} {}: {}", self.n_here, self.build_here, self.message),
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
    dir.join("estado.json")
}

pub fn write(dir: &Path, e: &State) -> Result<(), String> {
    let txt = serde_json::to_string_pretty(e).map_err(|x| x.to_string())?;
    std::fs::write(path(dir), txt).map_err(|x| format!("{}: {x}", path(dir).display()))
}

/// Reads the state. A missing or corrupt file returns `None` -- it never
/// brings down the next check.
pub fn read(dir: &Path) -> Option<State> {
    let txt = std::fs::read_to_string(path(dir)).ok()?;
    serde_json::from_str(&txt).ok()
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
        e.result = "atualizado".into();
        e.message = "trocado e conferido".into();
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
        let dir = std::env::temp_dir().join(format!("oxdesk-ota-lixo-{}", now()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(path(&dir), "nao e json").unwrap();
        assert!(read(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_file_returns_none() {
        assert!(read(Path::new("/tmp/nao-existe-oxdesk-ota-xyz")).is_none());
    }

    /// The wire names are a contract with the wizard and with the file
    /// the previous version wrote: renaming a Rust field must not move
    /// them.
    #[test]
    fn json_keys_keep_their_wire_names() {
        let txt = serde_json::to_string(&sample()).unwrap();
        for k in [
            "quando_unix", "sistema", "n_aqui", "build_aqui", "tag", "n_la", "build_la", "pacote",
            "resultado", "mensagem",
        ] {
            assert!(txt.contains(&format!("\"{k}\":")), "{k} missing in {txt}");
        }
    }

    /// Every result has to produce a phrase that says what happened.
    /// An empty line in the panel is the same as having no panel.
    #[test]
    fn every_result_has_a_phrase() {
        for r in ["igual", "atualizado", "novidade", "mais-novo-aqui", "revertido", "erro"] {
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
        for r in ["igual", "atualizado", "novidade", "mais-novo-aqui", "revertido", "erro"] {
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
        let dir = std::env::temp_dir().join(format!("oxdesk-ota-velho-{}", now()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            path(&dir),
            r#"{"quando_unix":1,"sistema":"linux","n_aqui":1,"build_aqui":"a","resultado":"igual","mensagem":"x"}"#,
        )
        .unwrap();
        let e = read(&dir).expect("the defaulted fields have to cover it");
        assert_eq!(e.n_there, 0);
        std::fs::remove_dir_all(&dir).ok();
    }
}
