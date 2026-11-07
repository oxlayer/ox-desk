//! What Linux and macOS do the same.
//!
//! It exists so there is no third copy of local time, config directory
//! and child builder. The difference between the two OSes is elsewhere --
//! journal vs. log file, systemd vs. launchd, mutter barrier vs. nothing
//! -- and that is where the modules diverge.

use std::path::PathBuf;
use std::process::Command;

/// Local time HH:MM:SS, without pulling in a date crate just for that.
pub fn hhmmss() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let t = (secs + offset_secs()).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

fn offset_secs() -> i64 {
    let Ok(out) = Command::new("date").arg("+%z").output() else { return 0 };
    let z = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if z.len() < 5 {
        return 0;
    }
    let sign = if z.starts_with('-') { -1 } else { 1 };
    let h: i64 = z[1..3].parse().unwrap_or(0);
    let m: i64 = z[3..5].parse().unwrap_or(0);
    sign * (h * 3600 + m * 60)
}

/// `$XDG_CONFIG_HOME/logi` or `~/.config/logi`, on both.
///
/// On macOS the native place would be `~/Library/Application Support`.
/// It stays `~/.config` on purpose: this config is edited by hand, and
/// the same path on both Unix machines is one less divergence between the
/// daemon and the GUI, which also has to get it right.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".config")
    });
    base.join("logi")
}

/// Starting point of the file browser.
/// Where the logs of processes we launch live.
///
/// `~/.local/state` is the same place as `logistream-host`'s log, so
/// whoever investigates a session looks at a single directory. Created
/// here because the caller is about to open a file inside it.
pub fn state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".local/state")
    });
    let _ = std::fs::create_dir_all(&base);
    base
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| "/".into())
}

/// Builds a `Command` for a child of the daemon.
///
/// There is nothing to adjust here; it exists so common code calls the
/// same builder on all three platforms. See the Windows version, where
/// the `CREATE_NO_WINDOW` flag avoids a flashing console window.

pub fn child(prog: &str) -> Command {
    Command::new(prog)
}
