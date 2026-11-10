// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `oxdesk-ota` -- the machine updates itself from the GitHub Releases.
//!
//! Spec: ../../specs/015-ota.md
//!
//! A separate binary, and not a thread inside `oxdesk`, for three reasons
//! that come together:
//!
//! 1. **Whoever swaps the binary must not die with it.** Restarting the
//!    daemon kills everything in the service's cgroup on Linux and in the
//!    job's process group under launchd. A thread inside `oxdesk` would
//!    not survive the very restart it asked for -- and nobody would be
//!    left to check `/healthz` or to roll back.
//! 2. **OTA has to work with the daemon dead.** That is exactly the case
//!    where updating fixes things. If the clock lived in `oxdesk`, a
//!    broken `oxdesk` would never fix itself again.
//! 3. The clock belongs to the system supervisor (systemd timer, launchd
//!    `StartInterval`, scheduled task), which already exists on all
//!    three machines and already wakes up everything that needs waking.
//!
//!   usage:  oxdesk-ota [--check | --force | --state]
//!
//! With no argument: check, and apply if there is something new.

mod apply;
mod exec;
mod plan;
mod release;
mod sha256;
mod state;

use std::process::ExitCode;

/// Translates the flags this binary answered to before they were renamed
/// to English.
///
/// Applied once, on the way in, so the parsing below reads exactly one
/// vocabulary.
///
/// Who still sends the old names, measured rather than assumed. The two
/// installed timers do **not**: the systemd unit and the Windows
/// scheduled task both invoke this binary with no arguments at all. What
/// does is `scripts/distribute-ota-token.sh`, which runs `--conferir`
/// over SSH on each machine, and a shell history. An unknown argument
/// exits 2, and the machine that script cannot update -- the Mac, while
/// it has no screen -- is exactly the one where a silent stop would go
/// unnoticed.
pub fn canonical(args: Vec<String>) -> Vec<String> {
    args.into_iter()
        .map(|a| {
            match a.as_str() {
                "--conferir" => "--check",
                "--forcar" => "--force",
                "--estado" => "--state",
                "--curto" => "--short",
                _ => return a,
            }
            .to_string()
        })
        .collect()
}

fn main() -> ExitCode {
    let args: Vec<String> = canonical(std::env::args().skip(1).collect());
    let mode = match args.first().map(String::as_str) {
        None => apply::Mode::Apply,
        Some("--check") => apply::Mode::Check,
        Some("--force") => apply::Mode::Force,
        Some("--state") => return show_state(args.iter().any(|a| a == "--short")),
        Some("-h") | Some("--help") => {
            println!(
                "oxdesk-ota [--check | --force | --state]

  (no argument)    check the latest Release and apply it if it is newer
  --check          only check and record the state; swaps nothing
  --force          apply even if the stamp is the same
  --state          print the JSON of the last check
  --state --short  one line: <result> TAB <age in s> TAB <phrase>

Read token for the Releases (private repo) in ~/.config/oxdesk/ota-token
or in the OXDESK_OTA_TOKEN variable. See specs/015-ota.md."
            );
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("oxdesk-ota: unknown argument {other}; try --help");
            return ExitCode::from(2);
        }
    };

    match apply::run(mode) {
        Ok(e) => {
            println!("{}: {}", e.result, e.line());
            ExitCode::SUCCESS
        }
        Err(e) => {
            // stderr: this runs from a scheduled task, and what matters is
            // that it lands in the supervisor's log.
            eprintln!("{}: {}", e.result, &e.message);
            ExitCode::FAILURE
        }
    }
}

/// The last check, for whoever consumes it.
///
/// `--short` exists for the wizard's `ota` step: `oxdesk` does not link
/// this crate (they are separate binaries) and **must not reimplement
/// either the file format or the phrases**. So the asker runs
/// `oxdesk-ota --state --short` and splits on TAB. One owner of the
/// format, a one-line contract.
fn show_state(short: bool) -> ExitCode {
    let ctx = match apply::context(apply::Mode::Check) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("oxdesk-ota: {e}");
            return ExitCode::FAILURE;
        }
    };
    match state::read(&ctx.dir) {
        Some(e) => {
            if short {
                println!("{}", state::short(&e, state::now()));
            } else {
                println!("{}", serde_json::to_string_pretty(&e).unwrap_or_default());
            }
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("nothing checked yet (nothing in {})", state::path(&ctx.dir).display());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A timer or a scheduled task written before the rename still passes
    /// the old flag, and an unknown argument exits 2 -- so dropping one
    /// stops the updater on whichever machine nobody is watching.
    #[test]
    fn the_old_flags_still_name_the_same_modes() {
        let got = canonical(
            ["--conferir", "--forcar", "--estado", "--curto"].map(String::from).to_vec(),
        );
        assert_eq!(got, ["--check", "--force", "--state", "--short"]);
    }

    #[test]
    fn a_name_we_do_not_translate_passes_through() {
        // Including the new names: translating twice must be a no-op, or
        // the table becomes order-dependent.
        let got = canonical(["--check", "--help", "-h", "banana"].map(String::from).to_vec());
        assert_eq!(got, ["--check", "--help", "-h", "banana"]);
    }
}
