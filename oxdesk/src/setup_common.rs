//! Setup checks that are the same on all three machines.
//!
//! Spec: ../../specs/011-onboarding.md
//!
//! It exists for the steps whose **criterion** does not change across
//! OSes -- does the config know this machine? does SSH reach the peers?
//! is the clipboard up? -- even if the mechanism underneath changes.
//! Where the mechanism differs, what differs is `crate::sys` or
//! `crate::remote`, which already exist for that.

use crate::config::Config;
use crate::setup::Check;
use std::time::Duration;

/// Runs a program with a time limit and returns its stdout.
///
/// A limit because this runs inside an HTTP request: a command that
/// hangs (and on macOS several hang when a permission is missing, instead
/// of failing) would block one of the server's four threads until someone
/// restarted the daemon.
pub fn run_timed(prog: &str, args: &[&str], seconds: u64) -> Result<String, String> {
    let mut child = crate::sys::child(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;

    // `wait_timeout` would be a dependency; this is a wait by polling
    // every 50 ms, which for a setup command is indistinguishable.
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{prog} ran past {seconds}s and was killed"));
            }
            Err(e) => return Err(format!("{prog}: {e}")),
        }
    }
    let output = child.wait_with_output().map_err(|e| format!("{prog}: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(text);
    }
    let err = String::from_utf8_lossy(&output.stderr);
    let line = err
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .or_else(|| text.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("failed with no message");
    Err(line.to_string())
}

/// The config knows this machine, and by which name.
pub fn this_machine_config(cfg: &Config) -> Check {
    let p = Config::path();
    let locals: Vec<&str> =
        cfg.hosts.iter().filter(|h| h.is_local()).map(|h| h.name.as_str()).collect();
    match locals.len() {
        1 => Check::ok("config", format!("{} is this machine ({})", locals[0], p.display())),
        0 => Check::missing("config", format!("no host marked as local in {}", p.display())),
        // Two "local" hosts make the panel, the border and input routing
        // disagree about who is "here", and each picks the first one it
        // finds -- which is not necessarily the same one.
        n => Check::missing("config", format!("{n} hosts marcados como local: {}", locals.join(", "))),
    }
}

/// SSH reaches each peer, without a password.
///
/// Measures in parallel because three hosts in series, with one down, add
/// up three timeouts -- and the wizard would measure in 12 s what it
/// measures in 4.
pub fn peers(cfg: &Config) -> Check {
    let targets: Vec<&crate::config::HostCfg> = cfg.hosts.iter().filter(|h| !h.is_local()).collect();
    if targets.is_empty() {
        return Check::not_applicable("peers", "no other host in the config");
    }
    let results: Vec<(String, Result<String, String>)> = std::thread::scope(|s| {
        let hs: Vec<_> = targets
            .iter()
            .map(|h| s.spawn(move || (h.name.clone(), crate::remote::run(h, "echo ok", 4))))
            .collect();
        hs.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    let bad: Vec<String> = results
        .iter()
        .filter(|(_, r)| !matches!(r, Ok(s) if s.contains("ok")))
        .map(|(n, r)| match r {
            Err(e) => format!("{n}: {e}"),
            // It answered, but not what we asked for: a shell that prints
            // a banner, a profile that writes to stdout, PowerShell that
            // does not wait.
            Ok(s) => format!("{n}: respondeu \"{}\"", s.trim().replace('\n', " ")),
        })
        .collect();

    if bad.is_empty() {
        Check::ok("peers", format!("{} of {} reachable", results.len(), targets.len()))
    } else {
        Check::missing("peers", bad.join("; "))
    }
}

/// Is the clipboard watcher up?
///
/// Reads the note the watcher itself leaves (`clip::watch_status`), instead of
/// testing from outside. Testing from outside would require writing to
/// the person's clipboard -- and a wizard that erases what you copied to
/// prove it works is worse than the problem.
/// Reads `tailscale status --json` and says whether this machine is on
/// the overlay.
///
/// The app counts as missing even with the overlay up: its extension,
/// without a login, trapped all of the Mac's traffic, and the VPN prompt
/// comes back every time it opens. See specs/012-outside-the-lan.md.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn tailscale_verdict(json: &str, app_installed: bool) -> Check {
    const ID: &str = "tailscale";
    if app_installed {
        return Check::missing(
            ID,
            "Tailscale.app is installed: delete it in Finder and deny the VPN prompt",
        );
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Check::error(ID, "`tailscale status` did not return JSON");
    };
    let ip = v["Self"]["TailscaleIPs"][0].as_str();
    match (v["BackendState"].as_str(), ip) {
        (Some("Running"), Some(ip)) => {
            let name = v["Self"]["HostName"].as_str().unwrap_or("?");
            Check::ok(ID, format!("on the tailnet as {name} ({ip})"))
        }
        (Some("Running"), None) => Check::error(ID, "connected, but with no tailnet address"),
        (Some("NeedsLogin"), _) => Check::missing(ID, "the service is up, the login is missing"),
        (Some(state), _) => Check::missing(ID, format!("Tailscale state: {state}")),
        (None, _) => Check::error(ID, "`tailscale status` reported no state"),
    }
}

/// Reads a count of present monitors, from whatever counted them.
///
/// The id is a parameter because the same question has a different answer
/// per OS: on Windows a machine with no monitor can grow one in software
/// (`virtual-display`), and on macOS it cannot -- someone has to plug
/// something in (`display`).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn monitors_verdict(id: &str, output: &str) -> Check {
    match output.trim().parse::<u32>() {
        Ok(0) => Check::missing(id, "no monitor present: the remote screen receives no frames"),
        Ok(n) => Check::ok(id, format!("{n} monitor(s) present")),
        Err(_) => Check::error(id, "could not count the monitors"),
    }
}

/// Is there a drawable display at all?
///
/// Separate from [`monitors_verdict`] because the number comes from an API
/// and not from a command's text, so there is nothing to parse -- and
/// because the two failures are different: `err != 0` means the question
/// could not be asked, which is [`State::Error`], never "no display".
///
/// Why this check exists: a MacBook whose panel broke reported **zero**
/// active displays while every other step on the list was green. The
/// screen host had its grant, was signed, was loaded -- and could not have
/// served a frame, because there was no framebuffer to read. Nothing on
/// the wizard said so. That is the silence the honesty rule is about.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn displays_verdict(id: &str, err: i32, count: u32) -> Check {
    if err != 0 {
        return Check::error(id, format!("CGGetActiveDisplayList: error {err}"));
    }
    match count {
        0 => Check::missing(
            id,
            "no active display: there is no framebuffer to capture, so the screen host \
             cannot serve a frame even with every permission granted",
        ),
        n => Check::ok(id, format!("{n} active display(s)")),
    }
}

pub fn clipboard() -> Check {
    match crate::clip::watch_status() {
        Some((true, d)) => Check::ok("clipboard", d),
        Some((false, d)) => Check::missing("clipboard", d),
        None => Check::error("clipboard", "the watcher has not reported anything yet"),
    }
}

// ------------------------------------------------------------------ repairs

/// Writes the default config, keeping the existing one in a copy.
pub fn write_config() -> Result<String, String> {
    let p = Config::path();
    if p.exists() {
        // Overwriting someone's config without a copy is destructive and
        // has no undo: this is where the Easy-Switch channels, the grid
        // and the SSH aliases live, all written by hand.
        let bak = p.with_extension("toml.bak");
        std::fs::copy(&p, &bak).map_err(|e| format!("copy to {}: {e}", bak.display()))?;
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let initial = crate::config::initial_config(
        crate::config::DEFAULT_TOML,
        &crate::config::local_hostname(),
        crate::config::local_os(),
    );
    std::fs::write(&p, initial)
        .map_err(|e| format!("write {}: {e}", p.display()))?;
    // Reopen before saying it worked: a default that does not parse would
    // leave the daemon without a config on the next start.
    Config::load().map_err(|e| format!("written, but it did not reopen: {e}"))?;
    Ok(format!("default config at {} (the previous one is at .toml.bak)", p.display()))
}

/// Forces a clipboard sample and returns what it measured.
pub fn test_clipboard() -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        // On Linux there is no sampling: the Mutter signal fires right
        // away, and the note already reflects the session. Restarting the
        // D-Bus thread to "test" would lose ownership of the current
        // selection -- what the person copied would vanish on the next
        // Ctrl+V.
        match crate::clip::watch_status() {
            Some((true, d)) => Ok(d),
            Some((false, d)) => Err(format!("{d}. Restart the daemon to try again.")),
            None => Err("the clipboard thread did not report: check oxdesk's log".into()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // One read now, through the same path as the watcher: it writes
        // the note, and we read the note. Reading directly here would
        // measure a different path from the one that actually runs.
        let _ = crate::clip_files::read_local();
        match crate::clip::watch_status() {
            Some((true, d)) => Ok(d),
            Some((false, d)) => Err(d),
            None => Err("the read reported nothing".into()),
        }
    }
}

/// `pgrep`/`tasklist` our way: does the process exist?
pub fn process_alive(name: &str) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("pgrep")
            .args(["-x", name])
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let _ = name;
        crate::sys::unit_state(name) == "active"
    }
}

// ---------------------------------------------------------------- OTA

/// "12 min ago", "3 h ago", "2 days ago" (pt-BR: shown on the panel).
///
/// `saturating_add` in the rounding: the age can arrive here as
/// `u64::MAX` when the field was unreadable, and a bare `+ 43_200` brings
/// the daemon down in a debug build. The test found it.
pub fn human_age(s: u64) -> String {
    match s {
        0..=89 => format!("{s}s ago"),
        90..=5399 => format!("{} min ago", s.saturating_add(30) / 60),
        5400..=172_799 => format!("{} h ago", s.saturating_add(1800) / 3600),
        _ => format!("{} days ago", s.saturating_add(43_200) / 86_400),
    }
}

/// How this machine's automatic update is doing.
///
/// It does not read `oxdesk-ota`'s `estado.json` nor rewrite its phrases:
/// it asks the binary itself (`--estado --curto`) and splits on TAB.
/// `oxdesk` and `oxdesk-ota` are separate binaries -- duplicating the file
/// format here would be the same trap as having the list of binaries in
/// two places ([[../../specs/013-ci-cd.md]]).
///
/// `clock` is what each OS measured from *its* scheduler: systemd timer,
/// launchd `StartInterval`, scheduled task.
pub fn ota(bin: &std::path::Path, token: &std::path::Path, clock: Result<String, String>) -> Check {
    if !bin.is_file() {
        return Check::missing("ota", format!("{} is not installed; run the deploy", bin.display()));
    }
    // The clock is THIS machine's configuration; the token is a
    // credential. Both can be missing at once, and reporting only one
    // hides the other: whoever reads the panel fixes what is written and
    // thinks they are done.
    //
    // The concrete case: without a token, all three machines say the
    // token is missing -- and that phrase would fit a machine with no
    // clock at all, which will never check anything even after the token
    // arrives.
    // It is a masked pending item, not a diagnosis error, and that is why
    // the message now adds both.
    let (clock, clock_missing) = match clock {
        Ok(t) => (t, None),
        Err(e) => (String::new(), Some(format!("the timer is not up: {e}"))),
    };
    let and_also = |s: String| match &clock_missing {
        Some(r) => format!("{s}; and {r}"),
        None => s,
    };

    // Without a token nothing works, and the symptom would be an API
    // "Not Found" that sends you looking at CI. So the missing token is
    // the diagnosis.
    if !token.is_file() {
        return Check::missing(
            "ota",
            and_also(format!(
                "the read token is missing at {}: the repo is private and the API answers \"Not Found\" without it",
                token.display()
            )),
        );
    }
    // A secret readable by any process on the machine is a leaked secret
    // -- and the Mac is precisely the one that leaves the house.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = std::fs::metadata(token) {
            let mode = m.permissions().mode() & 0o077;
            if mode != 0 {
                return Check::missing(
                    "ota",
                    and_also(format!(
                        "{} has mode {:o}: only the owner should read it (chmod 600)",
                        token.display(),
                        m.permissions().mode() & 0o777
                    )),
                );
            }
        }
    }
    if let Some(r) = clock_missing {
        return Check::missing("ota", r);
    }

    let line = match run_timed(&bin.to_string_lossy(), &["--estado", "--curto"], 8) {
        Ok(l) => l.trim().to_string(),
        // Exits != 0 when it never checked. That is a legitimate state
        // right after installing, not an error.
        Err(_) => return Check::missing("ota", format!("{clock}, but it has never checked yet")),
    };
    verdict(&line, &clock)
}

/// The decision, separate from whoever measures.
///
/// Pure because this is where the criterion lives -- and the criterion
/// has a case you only see when it is written down: **a last successful
/// check is not enough**. An hourly clock that has not woken up in six
/// hours stopped protecting, and since its last result was "up to date",
/// showing it as `Ok` says all is well precisely when it stopped being so.
pub fn verdict(line: &str, clock: &str) -> Check {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != 3 {
        return Check::error("ota", format!("cannot read oxdesk-ota's state: {line:?}"));
    }
    let (result, age, phrase) =
        (fields[0], fields[1].parse::<u64>().unwrap_or(u64::MAX), fields[2]);
    let when = human_age(age);

    match result {
        "erro" | "revertido" => Check::error("ota", format!("{when}: {phrase}")),
        _ if age > 6 * 3600 => Check::missing(
            "ota",
            format!("{clock}, but the last check was {when} -- the timer is not firing"),
        ),
        // An update published and not applied is pending, not success: on
        // Windows it is the permanent state (we do not sign there), and
        // that is exactly what the panel needs to show.
        "novidade" => Check::missing("ota", format!("{when}: {phrase}")),
        _ => Check::ok("ota", format!("{clock}; {when}: {phrase}")),
    }
}

// --------------------------------------- who is serving vs. what got installed

/// The stamp of the daemon that IS serving, against the one of the file
/// on disk.
///
/// `None` when they are the same build.
///
/// It exists because the Mac spent a whole day serving a ten-day-old
/// binary while **every** deploy said `ok`: the GUI started a `oxdesk` as
/// its own child, that child held the port, and the launchd copy died in
/// a loop with `Address already in use`. The new file got installed and
/// nobody ran it. The user was the one who noticed.
///
/// Comparing the two stamps turns a day of confusion into one panel line.
/// And it is the same hole OTA has: it restarts, asks `/healthz`, hears
/// the old build and **reverts a good update**.
pub fn stamp_verdict(live: (u64, &str), on_disk: (u64, &str)) -> Option<String> {
    let (n_live, b_live) = live;
    let (n_disk, b_disk) = on_disk;
    if b_live == b_disk {
        return None;
    }
    Some(format!(
        "the one answering is #{n_live} {b_live}, but the installed file is #{n_disk} {b_disk}: \
         some older process is holding the port"
    ))
}

/// Asks the installed binary for its stamp and compares it with mine.
///
/// I am the daemon serving this panel, so the "live" side is my own
/// compiled-in stamp -- I do not need to talk to the port to know who I
/// am.
///
/// A binary that does not understand `versao` returns `None`: during the
/// transition the honest answer is "I don't know", not an alarm.
pub fn daemon_on_disk(bin: &std::path::Path) -> Option<String> {
    // `sys::child` and not `Command::new`: on Windows a child process
    // without `CREATE_NO_WINDOW` opens a NEW console, and the panel is
    // read all the time -- it would be a cmd window flashing on screen.
    // That was complained about once already (see sys/windows.rs).
    let s = crate::sys::child(&bin.to_string_lossy()).arg("versao").output().ok()?;
    if !s.status.success() {
        return None;
    }
    let t = String::from_utf8_lossy(&s.stdout);
    let (n, b) = parse_stamp(&t)?;
    stamp_verdict(
        (env!("OXDESK_BUILD_N").parse::<u64>().unwrap_or(0), env!("OXDESK_BUILD")),
        (n, &b),
    )
}

/// `{"build":"abc1234","n":131}` -> `(131, "abc1234")`.
pub fn parse_stamp(json: &str) -> Option<(u64, String)> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    let b = v.get("build")?.as_str()?.to_string();
    let n = v.get("n")?.as_u64()?;
    Some((n, b))
}

/// `launchctl print` says the job is loaded -- but loaded is not
/// running.
///
/// `None` when it is really running.
///
/// The old check only failed the "process alive outside launchd" case and
/// passed the reverse: job loaded, `state = spawn scheduled`,
/// `last exit code = 1`, dying in a loop. That was exactly the Mac's
/// state while the panel said "loaded in the graphical session".
// Only the Mac has launchd; its test runs on any machine.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn launchd_verdict(print: &str) -> Option<String> {
    let field = |name: &str| -> Option<String> {
        print
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with(name) && l[name.len()..].trim_start().starts_with('='))
            .and_then(|l| l.split_once('='))
            .map(|(_, v)| v.trim().to_string())
    };
    let state = field("state").unwrap_or_default();
    if state.contains("running") {
        return None;
    }
    let output = field("last exit code").unwrap_or_default();
    let nonzero = !output.is_empty() && output != "0";
    if nonzero {
        return Some(format!(
            "loaded but not running: state = {state}, last exit code = {output} \
             (the port taken by another process is the usual cause)"
        ));
    }
    if state.is_empty() {
        // Without `state` in the print there is nothing to assert; do not
        // invent a failure.
        return None;
    }
    Some(format!("loaded but not running right now: state = {state}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::State;

    /// The panel phrase is read by a human: 5400 s is "an hour and a
    /// half", and showing "5400s" would make the reader divide in their
    /// head.
    #[test]
    fn age_in_words() {
        assert_eq!(human_age(0), "0s ago");
        assert_eq!(human_age(89), "89s ago");
        assert_eq!(human_age(90), "2 min ago");
        assert_eq!(human_age(3600), "60 min ago");
        assert_eq!(human_age(5400), "2 h ago");
        assert_eq!(human_age(90_000), "25 h ago");
        assert_eq!(human_age(172_800), "2 days ago");
    }

    /// A missing binary is the first question, before token and clock:
    /// without it neither of the other two makes sense.
    #[test]
    fn no_binary_does_not_ask_for_token() {
        let m = ota(
            std::path::Path::new("/does/not/exist/oxdesk-ota"),
            std::path::Path::new("/does/not/exist/token"),
            Ok("timer ok".into()),
        );
        assert_eq!(m.state, crate::setup::State::Missing);
        assert!(m.detail.contains("is not installed"), "{}", m.detail);
    }

    /// A stopped clock cannot show up as "ok" just because the last check
    /// succeeded: that is exactly the case where the machine stops
    /// updating and nothing warns.
    #[test]
    fn stopped_clock_is_not_ok_even_with_a_good_result() {
        let recent = verdict("igual\t600\tup to date on build #124 abc1234", "a cada 1 h");
        assert_eq!(recent.state, crate::setup::State::Ok);

        let stopped = verdict("igual\t25000\tup to date on build #124 abc1234", "a cada 1 h");
        assert_eq!(stopped.state, crate::setup::State::Missing);
        assert!(stopped.detail.contains("is not firing"), "{}", stopped.detail);
    }

    #[test]
    fn error_and_revert_are_errors_and_update_available_is_pending() {
        assert_eq!(verdict("erro\t60\tno token", "x").state, crate::setup::State::Error);
        assert_eq!(verdict("revertido\t60\trolled back", "x").state, crate::setup::State::Error);
        assert_eq!(verdict("novidade\t60\t#125 is out", "x").state, crate::setup::State::Missing);
        assert_eq!(verdict("atualizado\t60\tswapped", "x").state, crate::setup::State::Ok);
        assert_eq!(verdict("mais-novo-aqui\t60\t#130 here", "x").state, crate::setup::State::Ok);
    }

    /// A line outside the contract is an error, not "ok" by omission. If
    /// `oxdesk-ota` changes the format, the panel says so instead of lying.
    #[test]
    fn line_outside_the_contract_is_an_error() {
        let m = verdict("so um campo", "x");
        assert_eq!(m.state, crate::setup::State::Error);
        assert!(m.detail.contains("cannot read"), "{}", m.detail);
    }

    /// An unreadable age counts as old, never as fresh.
    #[test]
    fn unreadable_age_does_not_pass_as_fresh() {
        assert_eq!(verdict("igual\tabc\tfrase", "x").state, crate::setup::State::Missing);
    }
    /// Token and clock missing together must show up together.
    ///
    /// Without a token all three machines say the token is missing, and that
    /// phrase would fit a machine with no clock at all -- which will not
    /// check anything even after the token arrives. Whoever reads the
    /// panel fixes what is written and thinks they are done.
    #[test]
    fn missing_token_and_clock_both_show_up() {
        let m = ota(
            std::path::Path::new("/bin/sh"), // exists, to get past the first check
            std::path::Path::new("/does/not/exist/token"),
            Err("task 'ota' does not exist".into()),
        );
        assert_eq!(m.state, crate::setup::State::Missing);
        assert!(m.detail.contains("read token is missing"), "{}", m.detail);
        assert!(m.detail.contains("timer is not up"), "{}", m.detail);
        assert!(m.detail.contains("task 'ota' does not exist"), "{}", m.detail);
    }

    /// The Mac case: the GUI started a `oxdesk` as its child, the child
    /// held the port, and the launchd copy died in a loop. The two stamps
    /// diverge -- and that must reach the panel, because the user noticed
    /// it, not the deploy.
    #[test]
    fn diverging_stamp_shows_up() {
        let p = stamp_verdict((121, "2f1409d"), (131, "25512f1")).expect("has to fail");
        assert!(p.contains("#121 2f1409d"), "{p}");
        assert!(p.contains("#131 25512f1"), "{p}");
    }

    #[test]
    fn equal_stamp_stays_quiet() {
        assert!(stamp_verdict((131, "25512f1"), (131, "25512f1")).is_none());
    }

    /// A dirty tree installs its own dirty binary. Ignoring the suffix
    /// would make `abc` and `abc-sujo` pass as equal -- and it is between
    /// those two that the swap happens during a deploy from the laptop.
    #[test]
    fn dirty_is_not_the_same_as_clean() {
        assert!(stamp_verdict((131, "25512f1"), (131, "25512f1-sujo")).is_some());
    }

    #[test]
    fn unreadable_stamp_does_not_raise_an_alarm() {
        assert!(parse_stamp("{}").is_none());
        assert!(parse_stamp("not json").is_none());
        assert_eq!(parse_stamp("{\"build\":\"abc\",\"n\":7}\n"), Some((7, "abc".to_string())));
    }

    /// Real `launchctl print` output from the Mac while it served the old
    /// build. The panel said "loaded in the graphical session" because it only
    /// looked at whether the command succeeded.
    #[test]
    fn job_loaded_and_dying_is_not_ok() {
        let print = "\tstate = spawn scheduled\n\tpath = /Users/x/Library/LaunchAgents/app.oxdesk.daemon.plist\n\tlast exit code = 1\n";
        let p = launchd_verdict(print).expect("has to fail");
        assert!(p.contains("last exit code = 1"), "{p}");
        assert!(p.contains("spawn scheduled"), "{p}");
    }

    #[test]
    fn job_running_is_ok() {
        assert!(launchd_verdict("\tstate = running\n\tpid = 911\n\tlast exit code = 0\n").is_none());
    }

    /// `running` with a previous exit != 0 is normal: it fell once and
    /// came back. What is live now is what matters.
    #[test]
    fn running_now_beats_the_past_exit_code() {
        assert!(launchd_verdict("\tstate = running\n\tlast exit code = 1\n").is_none());
    }

    /// A field that only STARTS with the name is not the field.

    #[test]
    fn print_without_state_does_not_invent_a_failure() {
        assert!(launchd_verdict("\tpath = /x.plist\n\tstateful = sim\n").is_none());
    }

    #[test]
    fn tailscale_up_reports_name_and_address() {
        let m = tailscale_verdict(
            r#"{"BackendState":"Running","Self":{"HostName":"mac","TailscaleIPs":["100.1.2.3"]}}"#,
            false,
        );
        assert_eq!(m.state, State::Ok);
        assert!(m.detail.contains("100.1.2.3"));
    }

    #[test]
    fn tailscale_with_the_app_installed_is_missing_even_if_connected() {
        let m = tailscale_verdict(
            r#"{"BackendState":"Running","Self":{"TailscaleIPs":["100.1.2.3"]}}"#,
            true,
        );
        assert_eq!(m.state, State::Missing);
    }

    #[test]
    fn tailscale_without_login_is_missing_and_unmeasured_is_error() {
        let no_login = r#"{"BackendState":"NeedsLogin","Self":{}}"#;
        assert_eq!(tailscale_verdict(no_login, false).state, State::Missing);
        assert_eq!(tailscale_verdict("", false).state, State::Error);
        assert_eq!(tailscale_verdict("{}", false).state, State::Error);
        let no_ip = r#"{"BackendState":"Running","Self":{}}"#;
        assert_eq!(tailscale_verdict(no_ip, false).state, State::Error);
    }

    #[test]
    fn zero_monitors_is_missing_and_no_number_is_error() {
        assert_eq!(monitors_verdict("virtual-display", "1\r\n").state, State::Ok);
        assert_eq!(monitors_verdict("virtual-display", "0").state, State::Missing);
        assert_eq!(monitors_verdict("virtual-display", "").state, State::Error);
        assert_eq!(monitors_verdict("virtual-display", "Get-PnpDevice: error").state, State::Error);
    }

    /// A failed call is "I don't know", not "no display". Reporting zero
    /// on an error would tell someone to go buy a monitor they already
    /// have plugged in.
    #[test]
    fn a_failed_display_query_is_an_error_not_an_absence() {
        let m = displays_verdict("display", -1, 0);
        assert_eq!(m.state, State::Error);
        assert!(m.detail.contains("-1"), "{}", m.detail);
    }

    #[test]
    fn zero_displays_is_missing_and_one_is_ok() {
        let none = displays_verdict("display", 0, 0);
        assert_eq!(none.state, State::Missing);
        // The phrase has to name the consequence, because every other step
        // on that Mac was green and the screen still did not work.
        assert!(none.detail.contains("framebuffer"), "{}", none.detail);
        assert_eq!(displays_verdict("display", 0, 1).state, State::Ok);
        assert_eq!(displays_verdict("display", 0, 2).state, State::Ok);
    }
}
