// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The peers' state, kept in the background.
//!
//! It exists so the hot path does not wait on SSH. Measured: the agent
//! probe costs ~2.2 s, and it was 90% of the 3.9 s of a switch. On a
//! border crossing, waiting for that is unacceptable -- the user touches
//! the edge and wants it to happen.
//!
//! The refresher runs every `REFRESH`; the hot path reads the cache and
//! declares its age, so the check does not fake a freshness it lacks.

use crate::agent::{self, AgentInfo};
use crate::config::Config;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(6);

struct Entry {
    info: AgentInfo,
    at: Instant,
}

static CACHE: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);

/// The peer's state, and how many ms ago it was read.
pub fn cached(host: &str) -> Option<(AgentInfo, u64)> {
    let g = CACHE.lock().ok()?;
    let m = g.as_ref()?;
    let e = m.get(host)?;
    Some((
        AgentInfo {
            reachable: e.info.reachable,
            slots: e.info.slots.clone(),
            error: e.info.error.clone(),
        },
        e.at.elapsed().as_millis() as u64,
    ))
}

fn store(host: &str, info: AgentInfo) {
    if let Ok(mut g) = CACHE.lock() {
        g.get_or_insert_with(HashMap::new)
            .insert(host.to_string(), Entry { info, at: Instant::now() });
    }
}

/// Refreshes now, synchronously. Used at boot so the cache is not born
/// empty, and by whoever needs fresh data.

pub fn refresh(cfg: &Config) {
    for h in &cfg.hosts {
        if h.ssh.is_none() || h.agent.is_none() {
            continue;
        }
        store(&h.name, agent::devices(h));
    }
}

pub fn spawn(cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || loop {
        if let Ok(snap) = cfg.read().map(|g| g.clone()) {
            refresh(&snap);
        }
        std::thread::sleep(REFRESH);
    });
}
