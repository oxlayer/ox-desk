// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Moving an installation from the old names (logi, logid, logistream) to
//! oxdesk.
//!
//! Spec: ../../specs/017-product.md ("The name: oxdesk")
//!
//! Runs first thing in `main`, before the config is read. Every step moves
//! only when the new place does not exist yet and the old one does, so it is
//! idempotent and never overwrites: a second run, or a machine that was
//! migrated by the cutover script, finds nothing to do.
//!
//! What it covers is what the daemon itself reads: the config dir (with the
//! OTA token and, on the build machine, the signing key), the identity inside
//! it, and the HID++ slot-name cache. Binaries, services and scheduled tasks
//! belong to the installer and the cutover script, which can stop what is
//! running; a process cannot replace its own supervisor.

use std::path::{Path, PathBuf};

/// The moves, as (old, new). Pure, so it can be tested on a temp dir.
pub fn plan(config_base: &Path, cache_base: Option<&Path>) -> Vec<(PathBuf, PathBuf)> {
    let new_cfg = config_base.join("oxdesk");
    let id = new_cfg.join("identity");
    let mut v = vec![
        (config_base.join("logi"), new_cfg.clone()),
        (new_cfg.join("identidade"), id.clone()),
        (id.join("identidade.der"), id.join("identity.der")),
        (id.join("autorizados"), id.join("authorized")),
    ];
    if let Some(c) = cache_base {
        v.push((c.join("logi"), c.join("oxdesk")));
    }
    v
}

/// Applies the moves in order. Returns one line per move made, for the log.
pub fn apply(moves: &[(PathBuf, PathBuf)]) -> Vec<String> {
    let mut done = Vec::new();
    for (old, new) in moves {
        if new.exists() || !old.exists() {
            continue;
        }
        match std::fs::rename(old, new) {
            Ok(()) => done.push(format!("moved {} -> {}", old.display(), new.display())),
            Err(e) => done.push(format!("could not move {} -> {}: {e}", old.display(), new.display())),
        }
    }
    done
}

/// Values inside `config.toml` that name the old programs. Replaced as
/// whole quoted values, so comments and anything else stay as written.
const CONFIG_VALUES: &[(&str, &str)] = &[
    ("kind = \"logistream\"", "kind = \"oxdesk-stream\""),
    ("\"logid.service\"", "\"oxdesk.service\""),
    ("\"logi-ota.timer\"", "\"oxdesk-ota.timer\""),
    ("\"ai.logi.logid\"", "\"app.oxdesk.daemon\""),
    ("\"ai.logi.logistream-host\"", "\"app.oxdesk.host\""),
    ("\"ai.logi.ota\"", "\"app.oxdesk.ota\""),
    ("\\\\logid\\\\logid-agent.exe", "\\\\oxdesk\\\\oxdesk-agent.exe"),
    // Watched process names on Windows, and the agent path on the Unix peers.
    ("\"logid\"", "\"oxdesk\""),
    ("\"logid-agent\"", "\"oxdesk-agent\""),
    ("/.local/bin/logid\"", "/.local/bin/oxdesk\""),
];

/// The config text with the old values replaced, or `None` if nothing changed.
pub fn rewrite_config(text: &str) -> Option<String> {
    let mut out = text.to_string();
    for (old, new) in CONFIG_VALUES {
        out = out.replace(old, new);
    }
    (out != text).then_some(out)
}

/// The real run: this machine's config and cache bases.
pub fn migrate() -> Vec<String> {
    let config_dir = crate::sys::config_dir();
    let Some(config_base) = config_dir.parent() else { return vec![] };
    let cache_base: Option<PathBuf> = if cfg!(windows) {
        None
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    };
    let mut log = apply(&plan(config_base, cache_base.as_deref()));

    // The values, after the move: a backup first, then the new text.
    let cfg = config_dir.join("config.toml");
    if let Some(new) = std::fs::read_to_string(&cfg).ok().as_deref().and_then(rewrite_config) {
        let backup = config_dir.join("config.toml.before-oxdesk");
        let r = std::fs::copy(&cfg, &backup).and_then(|_| std::fs::write(&cfg, new));
        log.push(match r {
            Ok(()) => format!("rewrote the old program names in {} (backup {})", cfg.display(), backup.display()),
            Err(e) => format!("could not rewrite {}: {e}", cfg.display()),
        });
    }
    log
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oxdesk-legacy-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn moves_an_old_installation_and_keeps_the_contents() {
        let base = tmp("old");
        let old_id = base.join("cfg/logi/identidade");
        std::fs::create_dir_all(&old_id).unwrap();
        std::fs::write(base.join("cfg/logi/config.toml"), "port = 8787\n").unwrap();
        std::fs::write(base.join("cfg/logi/ota-token"), "x").unwrap();
        std::fs::write(old_id.join("identidade.der"), "key").unwrap();
        std::fs::write(old_id.join("autorizados"), "ab\n").unwrap();
        std::fs::write(old_id.join("pins"), "h ab\n").unwrap();
        std::fs::write(old_id.join("enforce"), "").unwrap();
        std::fs::create_dir_all(base.join("cache/logi")).unwrap();

        let log = apply(&plan(&base.join("cfg"), Some(&base.join("cache"))));
        assert_eq!(log.len(), 5, "{log:?}");
        let id = base.join("cfg/oxdesk/identity");
        assert_eq!(std::fs::read_to_string(base.join("cfg/oxdesk/config.toml")).unwrap(), "port = 8787\n");
        assert!(base.join("cfg/oxdesk/ota-token").exists());
        assert_eq!(std::fs::read_to_string(id.join("identity.der")).unwrap(), "key");
        assert_eq!(std::fs::read_to_string(id.join("authorized")).unwrap(), "ab\n");
        assert!(id.join("pins").exists() && id.join("enforce").exists(), "the switch survives");
        assert!(!base.join("cfg/logi").exists());
        assert!(base.join("cache/oxdesk").exists());

        // Idempotent: nothing left to do.
        assert!(apply(&plan(&base.join("cfg"), Some(&base.join("cache")))).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rewrites_only_the_old_values_in_the_config() {
        let old = concat!(
            "# logid config ON THIS MAC.\n",
            "units = [\"logid.service\"]\n",
            "agent = \"%LOCALAPPDATA%\\\\logid\\\\logid-agent.exe\"\n",
            "[hosts.screen]\n  kind = \"logistream\"\n",
            "units = [\"ai.logi.logid\"]\n",
            "units = [\"logid\", \"logid-agent\", \"sshd\"]\n",
            "agent = \"/home/alice/.local/bin/logid\"\n",
        );
        let new = rewrite_config(old).expect("changed");
        assert!(new.contains("units = [\"oxdesk.service\"]"), "{new}");
        assert!(new.contains("agent = \"%LOCALAPPDATA%\\\\oxdesk\\\\oxdesk-agent.exe\""), "{new}");
        assert!(new.contains("  kind = \"oxdesk-stream\""), "{new}");
        assert!(new.contains("units = [\"app.oxdesk.daemon\"]"), "{new}");
        assert!(new.contains("units = [\"oxdesk\", \"oxdesk-agent\", \"sshd\"]"), "{new}");
        assert!(new.contains("agent = \"/home/alice/.local/bin/oxdesk\""), "{new}");
        assert!(new.starts_with("# logid config ON THIS MAC."), "a comment without a quoted value stays");
        assert_eq!(rewrite_config(&new), None, "idempotent");
        // The result must still be a valid config for the new code.
        assert!(!new.contains("logistream\"\n  "), "{new}");
    }

    #[test]
    fn never_overwrites_a_new_installation() {
        let base = tmp("both");
        std::fs::create_dir_all(base.join("logi")).unwrap();
        std::fs::write(base.join("logi/config.toml"), "old").unwrap();
        std::fs::create_dir_all(base.join("oxdesk")).unwrap();
        std::fs::write(base.join("oxdesk/config.toml"), "new").unwrap();
        assert!(apply(&plan(&base, None)).is_empty());
        assert_eq!(std::fs::read_to_string(base.join("oxdesk/config.toml")).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&base);
    }
}
