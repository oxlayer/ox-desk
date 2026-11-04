//! Estado dos pares, mantido em background.
//!
//! Existe para o caminho quente nao esperar SSH. Medido: o probe do
//! agente custa ~2,2 s, e era 90% dos 3,9 s de uma troca. Numa travessia
//! de borda esperar por isso e inaceitavel -- o usuario encosta e quer
//! que aconteca.
//!
//! O refresher roda a cada `REFRESH`; o caminho quente le o cache e
//! declara a idade dele, para a checagem nao fingir frescor que nao tem.

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

/// Estado do par, e quantos ms atras foi lido.
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

/// Atualiza agora, sincronicamente. Usado no boot para o cache nao nascer
/// vazio, e por quem precisa de dado fresco.
pub fn refresh(cfg: &Config) {
    for h in &cfg.hosts {
        let (Some(alias), Some(exe)) = (&h.ssh, &h.agent) else { continue };
        store(&h.name, agent::devices(alias, exe));
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
