// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What differs between Linux and Windows, and only that.
//!
//! `oxdesk` was born on Linux only, and the GUI with it. Windows got a
//! one-shot CLI (`oxdesk-agent`), with no API, no tray, no window -- and
//! the Ubuntu GUI reached the other machine over SSH, calling the CLI by
//! hand. Result: switching was symmetric (`watch-edge` gave Windows the
//! border) but the control plane was not, and whoever was on Windows had
//! no panel at all.
//!
//! Measured before deciding: of the daemon's nine modules, six are
//! portable as they are. What tied it to Linux were five points --
//! `getent` in the DNS fallback, `systemctl` in the units' state, `date
//! +%z` for the timezone, `journalctl` as the log source and mutter's
//! D-Bus for the border. Hence this module: one place for the five,
//! instead of a second daemon in `agent/` duplicating the other six.
//!
//! The contract each platform fulfills:
//!
//! | function | Linux | macOS | Windows |
//! |---|---|---|---|
//! | `resolve_fallback` | `getent hosts` | nothing (native mDNS) | nothing (native mDNS) |
//! | `unit_state` | `systemctl --user` | `launchctl`, else process | service, else process |
//! | `hhmmss` | `date +%z` (shared) | same | `GetLocalTime` |
//! | `log_lines` | `journalctl --user -u` | file in `~/Library/Logs` | file in `%LOCALAPPDATA%` |
//! | `log_sink` | nothing (the journal timestamps) | appends with time | appends with time |
//! | `device_access` | ACL on `/dev/hidraw` | open the receiver | open the receiver |
//! | `spawn_border` | mutter barrier | **none** (see macos.rs) | `GetCursorPos` |
//!
//! What Linux and macOS do the same lives in `unix.rs`, so there is no
//! third copy of local time and config directory.

#[cfg(unix)]
mod unix;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

/// What a screen client can do, and how to ask for it.
///
/// It exists because the UI offers Parsec-style options -- keyboard and
/// mouse, sound, microphone -- and offering a checkbox that does nothing
/// is worse than not offering it. Each client declares what it honors;
/// what it does not honor shows up disabled with the reason.
///
/// What actually changes per client, measured from each one's docs:
///
/// | client | control | sound | microphone |
/// |---|---|---|---|
/// | `xfreerdp3` | always | `/sound` | `/microphone` |
/// | `gnome-connections` | always | not exposed | not exposed |
/// | `mstsc` | always | only via .rdp file | same |
/// | `vncviewer` | `-ViewOnly` turns it off | protocol lacks it | lacks it |
/// | `open vnc://` (macOS) | always | protocol lacks it | lacks it |
pub struct ClientSpec {
    pub prog: &'static str,
    pub args: &'static [&'static str],
    /// Extra arguments when sound is requested. `None` = cannot do it.
    pub audio: Option<&'static [&'static str]>,
    pub mic: Option<&'static [&'static str]>,
    /// Arguments to come in WITHOUT keyboard and mouse. `None` = the
    /// client always controls.
    pub view_only: Option<&'static [&'static str]>,
    /// Contents of an `.rdp` file the client consumes, when it does not
    /// take options on the command line.
    ///
    /// macOS's `Windows App` **refuses** the `rdp://` scheme (verified:
    /// "Unable to interpret ... as a path or URL") but opens an `.rdp`
    /// without complaint. Windows `mstsc` accepts both, and through the
    /// file it gets sound and microphone, which its command line does not
    /// offer.
    ///
    /// Markers: `{host}` `{port}` `{user_line}` `{audiomode}`
    /// `{audiocapture}`. The file's path goes into `args` as `{file}`.

    pub file: Option<&'static str>,
}

/// The file name of one of our binaries on this platform.
fn exe_name(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// Which path to use for a sibling binary: pure, so the decision has a
/// test. `dir` is the directory of the running executable and `present`
/// says whether the candidate is in it.
///
/// Falling back to the bare name rather than failing is deliberate: it
/// keeps PATH working for a developer who installed the binaries
/// somewhere else, and it is the behaviour this had before.
pub fn sibling_in(dir: Option<&std::path::Path>, name: &str, present: bool) -> std::path::PathBuf {
    match (dir, present) {
        (Some(d), true) => d.join(exe_name(name)),
        _ => std::path::PathBuf::from(exe_name(name)),
    }
}

/// The absolute path of one of our own binaries, resolved **next to this
/// one** instead of through `PATH`.
///
/// The bug it fixes, because it is not obvious from the symptom. The
/// daemon runs as a systemd user service, and that service's `PATH` is
/// systemd's own -- it does **not** contain `~/.local/bin`, which is
/// where the binaries are installed. So `Command::new("oxdesk-kvm")`
/// failed with ENOENT ("No such file or directory") about a file that was
/// right there, executable, and which the panel happily counted.
///
/// What made it look random: after `deploy.sh` the service is restarted
/// from a shell whose environment DOES have that directory, so the
/// gesture worked for the rest of the day and broke again at the next
/// boot. Reported as "sometimes it doesn't work, I just started the PC",
/// which is exactly right -- the PATH depended on who started the daemon.
///
/// `current_exe` is the right anchor: `deploy.sh`, `install.sh` and
/// `target/release` all keep these binaries side by side.
pub fn own_bin(name: &str) -> std::path::PathBuf {
    let exe = std::env::current_exe().ok();
    let dir = exe.as_deref().and_then(|p| p.parent());
    let present = dir.map(|d| d.join(exe_name(name)).exists()).unwrap_or(false);
    sibling_in(dir, name, present)
}

#[cfg(test)]
mod sibling_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_sibling_that_is_there_wins_over_the_path() {
        let got = sibling_in(Some(Path::new("/home/x/.local/bin")), "oxdesk-kvm", true);
        assert_eq!(got, Path::new("/home/x/.local/bin").join(exe_name("oxdesk-kvm")));
        assert!(got.is_absolute(), "an absolute path is the whole point: {got:?}");
    }

    /// Absent, or no idea where we are: the bare name, so PATH still gets
    /// its chance. Never an absolute path to a file we did not find.
    #[test]
    fn without_a_sibling_it_falls_back_to_the_bare_name() {
        assert_eq!(
            sibling_in(Some(Path::new("/home/x/.local/bin")), "oxdesk-kvm", false),
            Path::new(&exe_name("oxdesk-kvm"))
        );
        assert_eq!(sibling_in(None, "oxdesk-kvm", true), Path::new(&exe_name("oxdesk-kvm")));
    }

    /// On Windows the file has a suffix, and `PATHEXT` only covers the
    /// PATH lookup we are deliberately not using any more.
    #[test]
    fn the_windows_suffix_is_not_forgotten() {
        let n = exe_name("oxdesk-kvm");
        if cfg!(windows) {
            assert_eq!(n, "oxdesk-kvm.exe");
        } else {
            assert_eq!(n, "oxdesk-kvm");
        }
    }
}
