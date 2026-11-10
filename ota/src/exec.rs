// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Running programs and talking to the network, with a time limit.
//!
//! Spec: ../../specs/015-ota.md
//!
//! A twin of `rodar` in `oxdesk/src/setup_comum.rs`, on purpose: this
//! binary does not link the daemon (there is no `lib.rs` there) and
//! turning `oxdesk` into a library to share 40 lines of process waiting
//! would cost more than it pays. If a third client shows up, the right
//! place is a `common/` crate.
//!
//! Error strings end up in the state's message, which the panel shows:
//! they stay in Portuguese.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
fn command(prog: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(prog);
    // No window: this runs from a scheduled task, and a console flashing
    // in the middle of someone's session is exactly what the screen host
    // stopped doing (see the comment on the `.cmd` in deploy.sh).
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

#[cfg(not(windows))]
fn command(prog: &str) -> Command {
    Command::new(prog)
}

#[derive(Debug)]
pub struct Output {
    pub ok: bool,
    pub out: String,
    pub err: String,
}

/// Runs and waits, with a limit. Returns stdout, stderr and whether it
/// exited with 0.
///
/// Does not treat a non-zero code as an error: several steps here want
/// to **read** the output of a command that fails (`codesign` on an
/// ad-hoc binary, `curl` on a 404) and decide afterwards.
pub fn run(prog: &str, args: &[&str], seconds: u64) -> Result<Output, String> {
    run_with_stdin(prog, args, seconds, None)
}

/// The same, but writes something to the program's stdin before waiting.
///
/// It exists for one reason only: the GitHub token goes into `curl` via
/// `-K -`, never in `argv`. In `argv` it would show up in `ps` for every
/// process on the machine and in the logs of anyone who screenshots a
/// `ps aux`.
pub fn run_with_stdin(
    prog: &str,
    args: &[&str],
    seconds: u64,
    input: Option<&str>,
) -> Result<Output, String> {
    let mut child = command(prog)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;

    if let Some(txt) = input {
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(txt.as_bytes());
            // Close it: `curl -K -` reads the config until EOF.
        }
    }

    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(80)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{prog} passou de {seconds}s e foi morto"));
            }
            Err(e) => return Err(format!("{prog}: {e}")),
        }
    }

    let s = child.wait_with_output().map_err(|e| format!("{prog}: {e}"))?;
    Ok(Output {
        ok: s.status.success(),
        out: String::from_utf8_lossy(&s.stdout).into_owned(),
        err: String::from_utf8_lossy(&s.stderr).into_owned(),
    })
}

/// `curl`, and not a new TLS stack across three toolchains.
///
/// `curl` exists on all three machines (the daemon's `probe.rs` already
/// calls it, and on Windows as `curl.exe`, because `curl` there is an
/// alias for `Invoke-WebRequest`). Bringing in `reqwest`/`rustls` to
/// download one `.tar.gz` an hour would mean cross-compiling `ring` for
/// mingw and keeping our own certificate chain, to end up with the same
/// guarantee the system already gives.
fn curl_prog() -> &'static str {
    if cfg!(windows) {
        "curl.exe"
    } else {
        "curl"
    }
}

/// `curl` config carrying the token, to go in through stdin.
fn token_config(token: Option<&str>) -> Option<String> {
    let t = token?.trim().to_string();
    if t.is_empty() {
        return None;
    }
    Some(format!("header = \"Authorization: Bearer {t}\"\n"))
}

/// Fetches a text resource (the API's JSON response).
pub fn api(url: &str, token: Option<&str>, seconds: u64) -> Result<String, String> {
    let cfg = token_config(token);
    let s = run_with_stdin(
        curl_prog(),
        &[
            "-sS",
            "-L",
            "--max-time",
            &seconds.to_string(),
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "X-GitHub-Api-Version: 2022-11-28",
            "-K",
            "-",
            url,
        ],
        seconds + 10,
        cfg.as_deref(),
    )?;
    if !s.ok {
        // No `-f`: the body of a GitHub 404 says more than "HTTP 404".
        let reason = s.err.lines().next().unwrap_or("").trim();
        let body = s.out.trim();
        return Err(format!(
            "curl failed fetching {url}{}{}",
            if reason.is_empty() { String::new() } else { format!(": {reason}") },
            if body.is_empty() { String::new() } else { format!(" (corpo: {})", &body[..body.len().min(200)]) }
        ));
    }
    Ok(s.out)
}

/// Downloads a Release asset to a file.
///
/// `Accept: application/octet-stream` on the API address: on a private
/// repo the `browser_download_url` gives 404 even with a token.
pub fn download(url: &str, token: Option<&str>, dest: &Path, seconds: u64) -> Result<u64, String> {
    let cfg = token_config(token);
    let target = dest.to_string_lossy().into_owned();
    let s = run_with_stdin(
        curl_prog(),
        &[
            "-fsS",
            "-L",
            "--max-time",
            &seconds.to_string(),
            "-H",
            "Accept: application/octet-stream",
            "-o",
            &target,
            "-K",
            "-",
            url,
        ],
        seconds + 15,
        cfg.as_deref(),
    )?;
    if !s.ok {
        let _ = std::fs::remove_file(dest);
        let reason = s.err.lines().next().unwrap_or("failed with no message").trim();
        return Err(format!("did not download {url}: {reason}"));
    }
    let bytes = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    if bytes == 0 {
        return Err(format!("downloaded 0 bytes from {url}"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The token never goes into `argv`. This test guards the shape of
    /// what goes to curl's stdin -- if someone "simplifies" it to `-H`,
    /// the config stops existing and the test fails.
    #[test]
    fn the_token_becomes_stdin_config() {
        let c = token_config(Some("ghp_exemplo")).unwrap();
        assert_eq!(c, "header = \"Authorization: Bearer ghp_exemplo\"\n");
        assert!(token_config(None).is_none());
        assert!(token_config(Some("   ")).is_none(), "a blank token does not become a header");
        // The file's trailing newline does not go into the header.
        assert_eq!(token_config(Some("abc\n")).unwrap(), "header = \"Authorization: Bearer abc\"\n");
    }

    #[test]
    fn runs_and_returns_both_outputs() {
        #[cfg(unix)]
        {
            let s = run("sh", &["-c", "echo out; echo err >&2; exit 3"], 5).unwrap();
            assert!(!s.ok);
            assert!(s.out.contains("out"));
            assert!(s.err.contains("err"));
        }
    }

    /// A time limit is mandatory: this runs on its own, from a scheduled
    /// task, with nobody around to kill a hung command.
    #[test]
    fn the_limit_kills_whatever_hangs() {
        #[cfg(unix)]
        {
            let e = run("sh", &["-c", "sleep 30"], 1).unwrap_err();
            assert!(e.contains("foi morto"), "{e}");
        }
    }
}
