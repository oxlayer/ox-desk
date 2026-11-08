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
//!   usage:  oxdesk-ota [--conferir | --forcar | --estado]
//!
//! With no argument: check, and apply if there is something new.

mod apply;
mod exec;
mod plan;
mod release;
mod sha256;
mod state;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match args.first().map(String::as_str) {
        None => apply::Mode::Apply,
        Some("--conferir") => apply::Mode::Check,
        Some("--forcar") => apply::Mode::Force,
        Some("--estado") => return show_state(args.iter().any(|a| a == "--curto")),
        Some("-h") | Some("--help") => {
            println!(
                "oxdesk-ota [--conferir | --forcar | --estado]

  (no argument)    check the latest Release and apply it if it is newer
  --conferir       only check and record the state; swaps nothing
  --forcar         apply even if the stamp is the same
  --estado         print the JSON of the last check
  --estado --curto one line: <result> TAB <age in s> TAB <phrase>

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
            eprintln!("{}: {}", e.result, if e.result == "erro" { &e.message } else { &e.message });
            ExitCode::FAILURE
        }
    }
}

/// The last check, for whoever consumes it.
///
/// `--curto` exists for the wizard's `ota` step: `oxdesk` does not link
/// this crate (they are separate binaries) and **must not reimplement
/// either the file format or the phrases**. So the asker runs
/// `oxdesk-ota --estado --curto` and splits on TAB. One owner of the
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
