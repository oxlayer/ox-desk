//! What each system requires to receive a new build.
//!
//! Spec: ../../specs/015-ota.md
//!
//! Pure functions that return **data**: paths, names, the restart argv,
//! and the refusal reason when there is one. The one that executes is
//! `apply.rs`. The reason to split them is that the three machines
//! differ in everything -- where the binary lives, who restarts it,
//! whether it needs signing -- and that divergence has to be readable in
//! one place, not dug out of `cfg`s scattered around.

use std::path::{Path, PathBuf};

/// All three always exist, even in a build for just one of them: each
/// system's decision is read in one place and **tested for all three**
/// on this machine. One `cfg` per variant would make the Mac plan
/// unreadable from here.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    Linux,
    Mac,
    Windows,
}

/// The system of whoever is running.
pub fn current() -> System {
    #[cfg(target_os = "linux")]
    return System::Linux;
    #[cfg(target_os = "macos")]
    return System::Mac;
    #[cfg(windows)]
    return System::Windows;
}

/// The start of each system's package name.
///
/// Mirrors `PACOTE` in
/// [`build-binaries.sh`](../../scripts/build-binaries.sh). If
/// someone renames it there, this finds no package and the state says
/// exactly that -- instead of installing another architecture's package.
pub fn prefix(s: System) -> &'static str {
    match s {
        System::Linux => "oxdesk-linux-x86_64-",
        System::Mac => "oxdesk-mac-arm64-unsigned-",
        System::Windows => "oxdesk-windows-x86_64-unsigned-",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub system: System,
    /// The daemon's name inside the package. It is the one that carries
    /// the commit stamp and the one that answers `/healthz` after the
    /// swap.
    pub daemon: &'static str,
    /// Sign what was extracted before installing.
    ///
    /// Mac only. It is the step that makes the Screen Recording grant
    /// survive: TCC ties the permission to the binary's *designated
    /// requirement*, which the stable identity keeps the same. An ad-hoc
    /// binary from CI would come in with a new cdhash and lose the
    /// permission -- and the machine would come back with a black screen.
    pub signs: bool,
    /// Why this system does **not** swap binaries on its own, if so.
    pub refusal: Option<&'static str>,
    /// The argv that restarts the daemon.
    pub restart: Vec<String>,
    /// The argv that clears out of the way any daemon the supervisor does
    /// **not** know about, before restarting.
    ///
    /// It exists because `kickstart -k` (and `systemctl restart`) only
    /// kill the copy the supervisor created. An orphan `oxdesk` --
    /// typically started by the GUI in a boot race -- keeps the port, the
    /// new daemon dies with `Address already in use`, `/healthz` keeps
    /// answering the OLD build, and this program concludes the update
    /// failed and **rolls back a correct update**.
    ///
    /// Killing by exact name is not brutal: the `-k` of the next step
    /// kills the supervised copy anyway, so the net effect of this line
    /// is exactly "also the ones the supervisor does not know about".
    pub clear: Vec<String>,
}

pub fn plan(s: System, uid: u32) -> Plan {
    match s {
        System::Linux => Plan {
            system: s,
            daemon: "oxdesk",
            signs: false,
            refusal: None,
            restart: vec![
                "systemctl".into(),
                "--user".into(),
                "restart".into(),
                "oxdesk.service".into(),
            ],
            clear: vec!["pkill".into(), "-x".into(), "oxdesk".into()],
        },
        System::Mac => Plan {
            system: s,
            daemon: "oxdesk",
            signs: true,
            refusal: None,
            restart: vec![
                "launchctl".into(),
                "kickstart".into(),
                "-k".into(),
                format!("gui/{uid}/app.oxdesk.daemon"),
            ],
            clear: vec!["pkill".into(), "-x".into(), "oxdesk".into()],
        },
        // The Authenticode key never leaves the Ubuntu box (specs/008), and
        // an `.exe` without a valid signature is what Defender deleted the
        // instant it was written, twice, taking the scheduled task with
        // it. So here OTA **checks and reports**, and the one that swaps
        // is `deploy.sh`.
        //
        // Not a big loss: this machine does not travel. OTA exists for the
        // Mac, which does.
        System::Windows => Plan {
            system: s,
            daemon: "oxdesk.exe",
            signs: false,
            // Shown in the panel: stays in Portuguese.
            refusal: Some("I do not sign here: the Authenticode key lives on the Linux machine (specs/008); run the deploy"),
            restart: vec![],
            clear: vec![],
        },
    }
}

/// Where the installed binaries live.
///
/// `base` is `$HOME` on Unix and `%LOCALAPPDATA%` on Windows. It comes in
/// as a parameter so this is testable without touching the environment.
pub fn install_dir(s: System, base: &Path) -> PathBuf {
    match s {
        System::Linux | System::Mac => base.join(".local").join("bin"),
        System::Windows => base.join("oxdesk"),
    }
}

/// Where the state, the log and the copy of the previous build live.
pub fn state_dir(s: System, base: &Path) -> PathBuf {
    match s {
        System::Linux | System::Mac => base.join(".local").join("state").join("oxdesk-ota"),
        System::Windows => base.join("oxdesk").join("ota"),
    }
}

/// The signing script, when the system signs.
pub fn signer(s: System, base: &Path) -> Option<PathBuf> {
    match s {
        System::Mac => Some(base.join(".local").join("bin").join("sign.sh")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/Users/x")
    }

    /// Windows refuses, and **with a written reason**. A refusal without a
    /// reason would become "OTA does not work on Windows" with nobody
    /// knowing why.
    #[test]
    fn windows_refuses_and_says_why() {
        let p = plan(System::Windows, 501);
        let r = p.refusal.expect("Windows has to refuse");
        assert!(r.contains("Authenticode"), "{r}");
        assert!(p.restart.is_empty(), "whoever refuses restarts nothing");
    }

    #[test]
    fn only_the_mac_signs_and_has_a_signer() {
        assert!(plan(System::Mac, 501).signs);
        assert!(!plan(System::Linux, 1000).signs);
        assert!(!plan(System::Windows, 0).signs);
        assert!(signer(System::Mac, &home()).is_some());
        assert!(signer(System::Linux, &home()).is_none());
    }

    /// Whoever applies has to know how to restart. A plan that swaps the
    /// binary and does not restart leaves the machine running the old
    /// build with the new file on disk -- the hardest divergence to see.
    #[test]
    fn whoever_applies_knows_how_to_restart() {
        for s in [System::Linux, System::Mac, System::Windows] {
            let p = plan(s, 501);
            if p.refusal.is_none() {
                assert!(!p.restart.is_empty(), "{s:?} applies and does not restart");
            }
        }
    }

    /// Whoever applies has to know how to clear the old owner of the
    /// port. Restarting without that is what left the Mac serving an old
    /// build while every deploy said `ok`.
    #[test]
    fn whoever_applies_knows_how_to_clear_the_port() {
        for s in [System::Linux, System::Mac] {
            let p = plan(s, 501);
            assert!(!p.clear.is_empty(), "{s:?} applies and does not clear the port");
            assert_eq!(p.clear[0], "pkill", "{s:?}");
            // `-x`: EXACT name. Without it `pkill` would match `oxdesk-ota`
            // itself and any command line that mentions oxdesk.
            assert!(p.clear.contains(&"-x".to_string()), "{s:?}: {:?}", p.clear);
        }
        assert!(plan(System::Windows, 0).clear.is_empty(), "whoever refuses kills nothing");
    }

    /// launchd's `kickstart` needs the uid in the service path:
    /// `gui/501/...`. Without it the command fails with "Could not find
    /// service", which looks like a missing service.
    #[test]
    fn the_mac_restart_carries_the_uid() {
        let p = plan(System::Mac, 501);
        assert!(p.restart.iter().any(|a| a == "gui/501/app.oxdesk.daemon"), "{:?}", p.restart);
    }

    #[test]
    fn paths_are_absolute_and_differ_per_system() {
        let d: Vec<PathBuf> = [System::Linux, System::Mac, System::Windows]
            .iter()
            .map(|s| install_dir(*s, &home()))
            .collect();
        for p in &d {
            assert!(p.is_absolute(), "{p:?}");
        }
        assert_eq!(d[0], d[1], "Linux and Mac use the same ~/.local/bin");
        assert_ne!(d[1], d[2], "Windows does not use .local/bin");
        assert!(state_dir(System::Mac, &home()).ends_with("oxdesk-ota"));
    }

    /// Distinct prefixes, and none a prefix of another. `oxdesk-mac-arm64-`
    /// without `unsigned-` would match a future signed package, and that
    /// is wanted -- but none may match another system's package.
    #[test]
    fn prefixes_do_not_collide() {
        let ps = [
            prefix(System::Linux),
            prefix(System::Mac),
            prefix(System::Windows),
        ];
        for (i, a) in ps.iter().enumerate() {
            for (j, b) in ps.iter().enumerate() {
                if i != j {
                    assert!(!a.starts_with(b), "{a} starts with {b}");
                }
            }
        }
    }

    /// The Windows daemon is `.exe`; the others' is not. Getting this
    /// wrong would mean the stamp is never found in the right package.
    #[test]
    fn the_daemon_name_follows_the_system() {
        assert_eq!(plan(System::Windows, 0).daemon, "oxdesk.exe");
        assert_eq!(plan(System::Linux, 0).daemon, "oxdesk");
        assert_eq!(plan(System::Mac, 0).daemon, "oxdesk");
    }
}

/// Where the read token for the Releases lives.
///
/// The repo is private: without a token the API answers 200 with
/// `{"message":"Not Found"}`. The file is a fine-grained PAT with
/// `Contents: read` **on this repo and only this one** -- see
/// ../../specs/015-ota.md for what that changes in the trust model.
pub fn token_path(s: System, base: &Path) -> PathBuf {
    match s {
        System::Linux | System::Mac => base.join(".config").join("oxdesk").join("ota-token"),
        System::Windows => base.join("oxdesk").join("ota-token"),
    }
}
