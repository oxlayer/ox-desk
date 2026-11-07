//! Pairing, phase B: pin each host's identity, learned over SSH.
//!
//! Spec: ../../specs/017-product.md (section 1)
//!
//! SSH is already authenticated (`known_hosts`), so reading the host's
//! fingerprint through it is not trust on first use: whoever answers is the
//! machine whose host key we accepted. The same channel adds our own
//! fingerprint to the host's list of authorized clients, so phase C (the host
//! refusing strangers) can be turned on without anyone editing files.
//!
//! The pin is written once and compared afterwards. A different fingerprint
//! later means the other machine got a new identity (reinstall, wiped config)
//! or something is answering in its place; the session is refused and the
//! message says how to accept the new one on purpose.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::config::HostCfg;

const PINS_FILE: &str = "pins";

/// Same rule as `logistream::identity::dir`. Duplicated because logid does
/// not link the logistream crate; the two must agree, and the env override
/// exists for tests on both sides.
pub fn identity_dir() -> PathBuf {
    if let Ok(d) = std::env::var("LOGI_IDENTIDADE_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    #[cfg(windows)]
    let base = PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into()));
    #[cfg(not(windows))]
    let base = crate::sys::home_dir().join(".config");
    base.join("logi").join("identidade")
}

/// A SHA-256 in lowercase hex, and nothing else. Anything coming back from a
/// remote shell is checked against this before it is stored or sent anywhere.
pub fn is_fingerprint(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn short(fp: &str) -> &str {
    &fp[..fp.len().min(12)]
}

/// The pinned fingerprint for `host`, if any.
pub fn pinned(dir: &Path, host: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(PINS_FILE))
        .ok()?
        .lines()
        .filter_map(|l| l.split_once(' '))
        .find(|(name, _)| *name == host)
        .map(|(_, fp)| fp.trim().to_string())
        .filter(|fp| is_fingerprint(fp))
}

fn store_pin(dir: &Path, host: &str, fp: &str) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(dir).map_err(|e| format!("criar {}: {e}", dir.display()))?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(PINS_FILE))
        .map_err(|e| format!("gravar o pin: {e}"))?;
    writeln!(f, "{host} {fp}").map_err(|e| format!("gravar o pin: {e}"))
}

/// Decide what to do with the fingerprint the host reported.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// First time: pin it.
    New,
    /// Matches the pin.
    Same,
    /// Differs from the pin: refuse.
    Changed { pinned: String },
}

pub fn judge(pinned: Option<&str>, reported: &str) -> Verdict {
    match pinned {
        None => Verdict::New,
        Some(p) if p == reported => Verdict::Same,
        Some(p) => Verdict::Changed { pinned: p.to_string() },
    }
}

fn host_cmd(h: &HostCfg, args: &str) -> String {
    if crate::remote::is_powershell(h) {
        // `| Out-String` makes PowerShell wait for the GUI-subsystem exe
        // (rules/remote-powershell.md, trap #4).
        format!(r#"& "$env:LOCALAPPDATA\logid\logistream-host.exe" {args} | Out-String | Write-Output"#)
    } else {
        format!("~/.local/bin/logistream-host {args}")
    }
}

/// This machine's fingerprint, from the local host binary.
fn local_fingerprint(host_bin: &Path) -> Result<String, String> {
    let out = crate::sys::child(&host_bin.to_string_lossy())
        .arg("--digital")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ler a identidade local: {e}"))?;
    let fp = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !is_fingerprint(&fp) {
        return Err("a identidade local nao respondeu com uma digital".into());
    }
    Ok(fp)
}

fn remote_fingerprint(h: &HostCfg) -> Result<String, String> {
    let out = crate::remote::run(h, &host_cmd(h, "--digital"), 12)?;
    let fp = out.lines().map(str::trim).find(|l| is_fingerprint(l)).unwrap_or_default();
    if fp.is_empty() {
        return Err(format!(
            "{} nao respondeu com uma digital (o host de tela de la e anterior a 017?)",
            h.name
        ));
    }
    Ok(fp.to_string())
}

/// Add `fp` to the host's authorized list, unless it is already there.
/// `fp` was validated as 64 hex digits, so it is safe inside the command.
fn authorize_on(h: &HostCfg, fp: &str) -> Result<(), String> {
    debug_assert!(is_fingerprint(fp));
    let cmd = if crate::remote::is_powershell(h) {
        format!(
            concat!(
                r#"$d = "$env:APPDATA\logi\identidade"; "#,
                r#"New-Item -ItemType Directory -Force $d | Out-Null; $f = "$d\autorizados"; "#,
                r#"if (-not ((Test-Path $f) -and (Select-String -Path $f -Pattern '{fp}' -SimpleMatch -Quiet))) "#,
                r#"{{ Add-Content -Path $f -Value '{fp}' }}; "#,
                r#"if (Select-String -Path $f -Pattern '{fp}' -SimpleMatch -Quiet) {{ 'autorizado' }}"#
            ),
            fp = fp
        )
    } else {
        format!(
            concat!(
                r#"d="$HOME/.config/logi/identidade"; mkdir -p "$d" && chmod 700 "$d"; f="$d/autorizados"; "#,
                r#"grep -qx '{fp}' "$f" 2>/dev/null || echo '{fp}' >> "$f"; "#,
                r#"grep -qx '{fp}' "$f" && echo autorizado"#
            ),
            fp = fp
        )
    };
    let out = crate::remote::run(h, &cmd, 12)?;
    // Checked by reading back, not by the exit code of the append.
    if out.contains("autorizado") {
        Ok(())
    } else {
        Err(format!("nao consegui me autorizar em {}", h.name))
    }
}

/// The fingerprint the client must pin for `h`, pinning it on first use.
///
/// On first use this machine also authorizes itself on `h`. A failure there
/// is logged and does not block the session: phase C is not on yet, and the
/// next first-use (after deleting the pin) retries it.
pub fn pin_for(h: &HostCfg, host_bin: &Path) -> Result<String, String> {
    let dir = identity_dir();
    let reported = remote_fingerprint(h)?;
    match judge(pinned(&dir, &h.name).as_deref(), &reported) {
        Verdict::Same => Ok(reported),
        Verdict::Changed { pinned } => Err(format!(
            "a identidade de {} mudou: era {}, agora e {}. Se voce reinstalou ou apagou a config \
             la, tire a linha de {} em {} e conecte de novo",
            h.name,
            short(&pinned),
            short(&reported),
            h.name,
            dir.join(PINS_FILE).display()
        )),
        Verdict::New => {
            store_pin(&dir, &h.name, &reported)?;
            say!("pairing: pinned {} as {}", h.name, short(&reported));
            match local_fingerprint(host_bin).and_then(|me| authorize_on(h, &me).map(|_| me)) {
                Ok(me) => say!("pairing: {} now authorizes this machine ({})", h.name, short(&me)),
                Err(e) => say!("pairing: could not authorize this machine on {}: {e}", h.name),
            }
            Ok(reported)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "19c8a82d7d41aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "76dbb4057625bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn only_lowercase_sha256_hex_is_a_fingerprint() {
        assert!(is_fingerprint(A));
        assert!(!is_fingerprint(&A.to_uppercase()));
        assert!(!is_fingerprint(&A[..63]));
        assert!(!is_fingerprint("'; rm -rf ~; echo '0000000000000000000000000000000000000000000000"));
        assert!(!is_fingerprint(""));
    }

    #[test]
    fn first_use_pins_then_compares() {
        assert_eq!(judge(None, A), Verdict::New);
        assert_eq!(judge(Some(A), A), Verdict::Same);
        assert_eq!(judge(Some(A), B), Verdict::Changed { pinned: A.into() });
    }

    #[test]
    fn pins_are_per_host_and_survive_a_reload() {
        let d = std::env::temp_dir().join(format!("logid-pins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(pinned(&d, "ubuntu"), None);
        store_pin(&d, "ubuntu", A).unwrap();
        store_pin(&d, "windows-host", B).unwrap();
        assert_eq!(pinned(&d, "ubuntu").as_deref(), Some(A));
        assert_eq!(pinned(&d, "windows-host").as_deref(), Some(B));
        assert_eq!(pinned(&d, "mac"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
