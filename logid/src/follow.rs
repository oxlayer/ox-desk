//! Reconciliador: os dispositivos seguem o canal do lider.
//!
//! Spec: ../specs/005-input-switch.md
//!
//! Resolve o estado dividido na raiz -- mouse numa maquina, teclado em
//! outra -- e funciona inclusive quando a troca foi **manual**, no botao
//! Easy-Switch, que a vigilancia de borda nao ve.
//!
//! A assimetria do hardware manda no desenho: so quem detem o dispositivo
//! pode empurra-lo. Entao:
//!
//! | onde esta o seguidor | quem age |
//! |---|---|
//! | aqui | nos mesmos, por HID++ local |
//! | num par | pedimos ao agente dele, por SSH |
//! | em nenhum dos dois | nada a fazer (canal desconhecido, ex. MacBook) |

use crate::agent;
use crate::config::{Config, HostCfg};
use crate::focus;
use crate::hidpp;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Espera depois de agir, para nao brigar com a borda nem consigo mesmo.
const AFTER_ACTION: Duration = Duration::from_secs(8);

fn is_local(h: &HostCfg) -> bool {
    h.addr == "127.0.0.1" || h.addr == "localhost" || h.addr == "::1"
}

/// Nosso canal, pelo mapa da config. Preferimos isto a ler do hardware:
/// funciona mesmo com todos os dispositivos ausentes.
fn my_channel(cfg: &Config) -> Option<(String, u8)> {
    let me = cfg.hosts.iter().find(|h| is_local(h))?;
    let ch = cfg
        .devices
        .iter()
        .find_map(|d| d.hosts.get(&me.name).copied())?;
    Some((me.name.clone(), ch))
}

struct Seen {
    /// slot local, se estiver aqui
    local_slot: Option<u8>,
    /// canal em que o dispositivo esta, se descobrimos
    channel: Option<u8>,
    /// par que o detem (nome do host), se nao for aqui
    holder: Option<String>,
    /// slot no par que o detem
    holder_slot: Option<u8>,
}

/// Onde esta cada dispositivo de `[[devices]]`, consultando daqui e dos pares.
fn locate(cfg: &Config) -> std::collections::BTreeMap<String, Seen> {
    let mut out: std::collections::BTreeMap<String, Seen> = cfg
        .devices
        .iter()
        .map(|d| {
            (
                d.name.clone(),
                Seen { local_slot: None, channel: None, holder: None, holder_slot: None },
            )
        })
        .collect();

    // Local, por HID++.
    let local = hidpp::survey();
    for dev in local.receivers.iter().flat_map(|r| r.devices.iter()) {
        if let Some(s) = out.get_mut(&dev.name) {
            if dev.online {
                s.local_slot = Some(dev.slot);
                s.channel = dev.current_host;
            }
        }
    }

    // Pares, por SSH. O agente reporta slot e canal, mas NAO o nome --
    // por isso casamos por canal, nao por nome: se o par ve um dispositivo
    // no canal dele, e aquele que esta la.
    for h in cfg.hosts.iter().filter(|h| !is_local(h)) {
        let (Some(alias), Some(exe)) = (&h.ssh, &h.agent) else { continue };
        let info = agent::devices(alias, exe);
        if !info.reachable {
            continue;
        }
        for slot in info.slots.iter().filter(|s| s.online) {
            // Casamos por NOME. Casar por canal era ambiguo: mouse e
            // teclado declaram o mesmo canal para o mesmo host.
            let Some(name) = slot.name.as_deref() else { continue };
            let Some(seen) = out.get_mut(name) else { continue };
            if seen.local_slot.is_some() {
                continue; // esta aqui; o par nao pode te-lo
            }
            seen.channel = slot.current_host;
            seen.holder = Some(h.name.clone());
            seen.holder_slot = Some(slot.slot);
        }
    }
    out
}

/// Uma passada de reconciliacao. Devolve o que fez, para o log.
fn reconcile(cfg: &Config) -> Vec<String> {
    let mut acted = Vec::new();
    let Some(leader_name) = cfg.input.follow.leader.clone() else {
        return acted;
    };
    let seen = locate(cfg);
    let Some(leader) = seen.get(&leader_name) else {
        return vec![format!("lider '{leader_name}' nao esta em [[devices]]")];
    };
    let Some(target_channel) = leader.channel else {
        // Sem saber onde o lider esta, nao movemos nada. Mover para canal
        // errado deixa o dispositivo inalcancavel das duas maquinas.
        return acted;
    };

    for (name, s) in seen.iter() {
        if name == &leader_name || s.channel == Some(target_channel) {
            continue;
        }
        let Some(dcfg) = cfg.devices.iter().find(|d| &d.name == name) else { continue };

        if let Some(slot) = s.local_slot {
            // Nos detemos: agimos direto.
            match hidpp::change_host(slot, target_channel) {
                Ok(detail) => acted.push(format!(
                    "{name} seguiu o lider para o canal {target_channel}: {detail}"
                )),
                Err(e) => acted.push(format!("{name}: FALHOU ao seguir -- {e}")),
            }
        } else if let (Some(holder), Some(hslot)) = (&s.holder, s.holder_slot) {
            // Um par detem: pedimos a ele.
            let Some(hc) = cfg.hosts.iter().find(|h| &h.name == holder) else { continue };
            let (Some(alias), Some(exe)) = (&hc.ssh, &hc.agent) else { continue };
            match agent::set_host(alias, exe, hslot, target_channel) {
                Ok(()) => acted.push(format!(
                    "{name}: pedi a {holder} que o mandasse para o canal {target_channel}"
                )),
                Err(e) => acted.push(format!("{name}: {holder} nao conseguiu mandar -- {e}")),
            }
        } else {
            // Nem aqui nem num par conhecido: pode estar no MacBook, que
            // ainda nao tem daemon. Nao ha o que fazer.
            let _ = dcfg;
        }
    }
    acted
}

pub fn spawn(cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || {
        let (enabled, leader, interval) = {
            let Ok(g) = cfg.read() else { return };
            let f = &g.input.follow;
            (f.enabled, f.leader.clone(), f.interval_seconds.max(2))
        };
        if !enabled {
            return;
        }
        if leader.is_none() {
            eprintln!("follow: habilitado sem `leader`; nada a fazer");
            return;
        }
        println!(
            "follow: reconciliando a cada {interval}s; lider = {}",
            leader.unwrap_or_default()
        );

        loop {
            std::thread::sleep(Duration::from_secs(interval));
            let Ok(snap) = cfg.read().map(|g| g.clone()) else { continue };
            if !snap.input.follow.enabled {
                continue;
            }
            // Mesmo lock das trocas: nunca competir com a borda nem com a API.
            let acted = {
                let _g = focus::switch_lock();
                reconcile(&snap)
            };
            if !acted.is_empty() {
                for a in &acted {
                    println!("follow: {a}");
                }
                std::thread::sleep(AFTER_ACTION);
            }
        }
    });
}
