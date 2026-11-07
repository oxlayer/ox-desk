//! What differs between Linux and Windows, and only that.
//!
//! `logid` was born on Linux only, and the GUI with it. Windows got a
//! one-shot CLI (`logid-agent`), with no API, no tray, no window -- and
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
