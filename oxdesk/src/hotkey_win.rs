// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The shortcut on Windows: the key reader. The decision is `hotkey::fire`,
//! shared with Linux.
//!
//! Spec: ../../specs/023-shortcut.md, ../../specs/024-windows-kvm-source.md
//!
//! Until spec 024 the shortcut existed on Linux only, because only Linux
//! could be the KVM's source. With the receiver on Windows, both Ctrl did
//! nothing at all -- reported as "I'm hitting ctrl+ctrl but it is not
//! going to ubuntu".
//!
//! `winput::keys` watches without taking. Three rules keep it from doing
//! the wrong thing:
//!
//! - **Injected keys are ignored.** When another machine forwards its
//!   keyboard here, our host types it with `SendInput`; the way back for
//!   that keyboard is the rescue in the SOURCE machine's `oxdesk-kvm`, and
//!   this reader answering too would bounce the input straight back.
//! - **Edge, not level**, with the same cooldown as Linux: holding the pair
//!   auto-repeats, and one press must be one switch.
//! - **The callback only records and signals.** Windows drops a slow
//!   low-level hook without a word; `fire` (which waits on `kvm::send`)
//!   runs on a worker thread.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use crate::config::Config;
use crate::hotkey::{chord_down, chord_key_from_vk, fire, COOLDOWN};

struct Seen {
    pressed: HashSet<u16>,
    down: bool,
    chord: (u16, u16),
    last_fired: Instant,
}

pub fn start(cfg: Arc<RwLock<Config>>, cache: Arc<Mutex<crate::Cache>>) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let seen = Mutex::new(Seen {
        pressed: HashSet::new(),
        down: false,
        chord: crate::config::CHORDS[0].1,
        last_fired: Instant::now() - COOLDOWN,
    });
    let cfg_hook = Arc::clone(&cfg);
    let watched = winput::keys::watch_keys(Box::new(move |k| {
        if k.injected {
            return;
        }
        let Some(code) = chord_key_from_vk(k.vk, k.scan) else { return };
        let Ok(mut s) = seen.lock() else { return };
        // Read on every chord key, so a change in the panel takes effect
        // without a restart; a new pair starts from nothing held.
        let chord = cfg_hook.read().map(|g| g.input.hotkey.chord()).unwrap_or(s.chord);
        if chord != s.chord {
            s.chord = chord;
            s.pressed.clear();
            s.down = false;
        }
        if code != chord.0 && code != chord.1 {
            return;
        }
        if k.down {
            s.pressed.insert(code);
        } else {
            s.pressed.remove(&code);
        }
        let now_down = chord_down(&s.pressed, chord);
        if now_down && !s.down && s.last_fired.elapsed() >= COOLDOWN {
            s.last_fired = Instant::now();
            let _ = tx.send(());
            // Forget the pair at once. If the switch works, the fingers come
            // up into `oxdesk-kvm`'s hook and this one never sees the
            // key-ups: kept, the pair would stay "down" and the next press
            // would be no edge at all -- the one dead press per switch that
            // the Linux reader paid for (`hotkey::prune_stale`). Auto-repeat
            // only repeats the last key, so one held pair cannot refire.
            s.pressed.clear();
            s.down = false;
            return;
        }
        s.down = now_down;
    }));
    match watched {
        Ok(()) => crate::say!("hotkey: watching the keyboard for the shortcut"),
        Err(e) => {
            crate::gripe!("hotkey: the shortcut is off on this machine -- {e}");
            return;
        }
    }
    std::thread::Builder::new()
        .name("hotkey-fire".into())
        .spawn(move || {
            for () in rx {
                fire(&cfg, &cache);
            }
        })
        .ok();
}
