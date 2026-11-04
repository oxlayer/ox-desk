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
//!
//! ## 04/11: por que ele agora CONFERE e DESISTE
//!
//! Este modulo deixou o usuario sem poder trabalhar. O log:
//!
//! ```text
//! 08:18:30  MX Anywhere 3: pedi a windows-host que o mandasse para o canal 1
//! 08:18:56  ... para o canal 1
//! 08:19:17  ... para o canal 1      (sete vezes em dois minutos)
//! ```
//!
//! Duas falhas somadas, e a segunda e a grave:
//!
//! 1. `hidpp::set_host` devolve `Ok` quando o dispositivo nao responde --
//!    e ali a ausencia de resposta e de fato o caso normal (o radio cai
//!    antes da resposta). Entao "pedi ao par" NUNCA significou "foi".
//! 2. Ele nao conferia o resultado e nao tinha limite. Pedir a mesma
//!    coisa sete vezes nao e insistencia, e um laco -- e um laco que
//!    mexe no teclado de alguem e pior que um erro.
//!
//! Agora: depois de agir ele CONFERE se o dispositivo mudou de lugar, e
//! tres tentativas sem efeito **desligam o reconciliador** e dizem isso
//! no log e na UI. Erro barulhento e melhor que retry silencioso -- a
//! mesma licao do langfuse e da `backfill_gate` no outro repo.

use crate::agent;
use crate::config::Config;
use crate::focus;
use crate::hidpp;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Espera depois de agir, para nao brigar com a borda nem consigo mesmo.
const AFTER_ACTION: Duration = Duration::from_secs(8);

/// Carencia depois que os dispositivos chegam aqui: nao reconciliar
/// sobre estado em transito. Mesmo numero da borda, pelo mesmo motivo.
const SETTLE: u64 = 7_000;

/// Tentativas sem efeito, no mesmo dispositivo, antes de se desligar.
///
/// Tres e nao dez porque cada tentativa mexe no teclado de alguem. Se
/// duas nao resolveram, a terceira tambem nao vai -- o que esta errado e
/// o nosso modelo do mundo, nao a sorte.
const LIMITE_FALHAS: u32 = 3;

/// Quanto esperar entre a acao e a conferencia.
///
/// Uma troca de canal leva ~120 ms para o dispositivo sair do receptor,
/// mas ate ~1,5 s para aparecer no outro. Conferir antes disso leria
/// "nao chegou" para uma troca que estava a caminho.
const CONFERIR_APOS: Duration = Duration::from_millis(1_800);

/// Espera depois de uma tentativa que NAO se confirmou.
///
/// Mais longa que `AFTER_ACTION` de proposito: se nao funcionou, repetir
/// rapido so multiplica o estrago.
const APOS_FALHA: Duration = Duration::from_secs(30);

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
    for h in cfg.hosts.iter().filter(|h| !h.is_local()) {
        // So faz sentido perguntar a quem tem canal e agente declarados.
        if h.ssh.is_none() || h.agent.is_none() { continue }
        let info = agent::devices(h);
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

/// O que saiu de uma tentativa, para o disjuntor poder decidir.
pub enum Resultado {
    /// Conferido: o dispositivo saiu de onde estava.
    Moveu(String),
    /// Pedimos, ninguem reclamou, e nada aconteceu. O caso que virou laco.
    SemEfeito(String),
    Erro(String),
}

impl Resultado {
    fn texto(&self) -> &str {
        match self {
            Resultado::Moveu(t) | Resultado::SemEfeito(t) | Resultado::Erro(t) => t,
        }
    }
    fn ok(&self) -> bool {
        matches!(self, Resultado::Moveu(_))
    }
}

/// O dispositivo saiu mesmo do slot do par?
///
/// Pergunta de novo ao agente dele. Se o par nao responder, tratamos como
/// "nao saiu": na duvida, NAO contar como sucesso -- contar sucesso falso
/// e exatamente o que produziu o laco de sete pedidos.
fn saiu_do_par(h: &crate::config::HostCfg, slot: u8) -> bool {
    let info = agent::devices(h);
    if !info.reachable {
        return false;
    }
    !info.slots.iter().any(|s| s.slot == slot && s.online)
}

/// Uma passada de reconciliacao. Devolve o que fez, por dispositivo.
fn reconcile(cfg: &Config) -> Vec<(String, Resultado)> {
    let mut acted: Vec<(String, Resultado)> = Vec::new();
    let Some(leader_name) = cfg.input.follow.leader.clone() else {
        return acted;
    };
    let seen = locate(cfg);
    let Some(leader) = seen.get(&leader_name) else {
        return vec![(
            leader_name.clone(),
            Resultado::Erro(format!("lider '{leader_name}' nao esta em [[devices]]")),
        )];
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
            // Nos detemos: agimos direto. Este caminho JA confere -- o
            // `change_host` espera o dispositivo sair do receptor.
            let r = match hidpp::change_host(slot, target_channel) {
                Ok(detail) => Resultado::Moveu(format!(
                    "{name} seguiu o lider para o canal {target_channel}: {detail}"
                )),
                Err(e) => Resultado::Erro(format!("{name}: FALHOU ao seguir -- {e}")),
            };
            acted.push((name.clone(), r));
        } else if let (Some(holder), Some(hslot)) = (&s.holder, s.holder_slot) {
            // Um par detem: pedimos a ele -- e CONFERIMOS, porque o
            // `set_host` do outro lado devolve Ok sem resposta do radio.
            let Some(hc) = cfg.hosts.iter().find(|h| &h.name == holder) else { continue };
            if hc.ssh.is_none() || hc.agent.is_none() { continue }
            let r = match agent::set_host(hc, hslot, target_channel) {
                Ok(()) => {
                    std::thread::sleep(CONFERIR_APOS);
                    if saiu_do_par(hc, hslot) {
                        Resultado::Moveu(format!(
                            "{name}: {holder} o mandou para o canal {target_channel}"
                        ))
                    } else {
                        Resultado::SemEfeito(format!(
                            "{name}: pedi a {holder} o canal {target_channel} e ele NAO saiu \
                             de la -- o pedido foi aceito sem efeito"
                        ))
                    }
                }
                Err(e) => Resultado::Erro(format!("{name}: {holder} nao conseguiu mandar -- {e}")),
            };
            acted.push((name.clone(), r));
        } else {
            // Nem aqui nem num par conhecido: pode estar no MacBook, que
            // ainda nao tem daemon. Nao ha o que fazer.
            let _ = dcfg;
        }
    }
    acted
}

/// O que fazer depois de uma passada.
#[derive(Debug, PartialEq, Eq)]
pub enum Acao {
    /// Nada foi feito; so esperar o intervalo.
    Nada,
    /// Agiu e confirmou. Carencia curta para nao brigar consigo mesmo.
    Esperar,
    /// Agiu, nao confirmou. Carencia longa: repetir rapido multiplica o
    /// estrago.
    EsperarMais,
    /// Limite estourado: desligar o reconciliador e dizer por que.
    Desligar { dev: String, tentativas: u32 },
}

/// Conta as falhas e decide. Pura de proposito: o disjuntor e a parte que
/// nao pode estar errada, e um laco com `sleep` dentro nao se testa.
///
/// Muta `falhas` porque a contagem e POR DISPOSITIVO e atravessa
/// passadas: um sucesso zera aquele dispositivo sem perdoar os outros.
fn avaliar(
    acted: &[(String, Resultado)],
    falhas: &mut std::collections::BTreeMap<String, u32>,
) -> Acao {
    if acted.is_empty() {
        return Acao::Nada;
    }
    let mut houve_falha = false;
    let mut estourou: Option<(String, u32)> = None;
    for (dev, r) in acted {
        if r.ok() {
            falhas.remove(dev);
            continue;
        }
        houve_falha = true;
        let n = falhas.entry(dev.clone()).or_insert(0);
        *n += 1;
        if *n >= LIMITE_FALHAS && estourou.is_none() {
            estourou = Some((dev.clone(), *n));
        }
    }
    match estourou {
        Some((dev, tentativas)) => Acao::Desligar { dev, tentativas },
        None if houve_falha => Acao::EsperarMais,
        None => Acao::Esperar,
    }
}

pub fn spawn(cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || {
        // O laco roda SEMPRE e le o estado a cada passada, em vez de
        // sair quando esta desligado no boot. Antes, ligar pela UI nao
        // tinha efeito ate reiniciar o daemon -- e a resposta da API
        // dizia "vale no proximo boot", o que fez o usuario desconfiar
        // do interruptor justamente quando precisava dele.
        let mut falhas: std::collections::BTreeMap<String, u32> = Default::default();
        let mut anunciado = false;

        loop {
            let intervalo = cfg
                .read()
                .map(|g| g.input.follow.interval_seconds.max(2))
                .unwrap_or(5);
            std::thread::sleep(Duration::from_secs(intervalo));

            let Ok(snap) = cfg.read().map(|g| g.clone()) else { continue };
            if !snap.input.follow.enabled {
                // Desligado: o estado anterior nao vale mais. Religar
                // depois de resolver o problema comeca do zero, em vez de
                // herdar tres falhas antigas e se desligar na primeira.
                falhas.clear();
                anunciado = false;
                continue;
            }
            let Some(lider) = snap.input.follow.leader.clone() else {
                if !anunciado {
                    gripe!("follow: ligado sem `leader`; nada a fazer");
                    anunciado = true;
                }
                continue;
            };
            if !anunciado {
                say!("follow: reconciliando a cada {intervalo}s; lider = {lider}");
                anunciado = true;
            }

            // Logo depois de uma chegada o estado esta em transito, e o
            // reconciliador leria uma foto ja velha. O log do usuario
            // mostrou o estrago: `follow: pedi a windows-host que o
            // mandasse para o canal 1` onze segundos antes de a borda
            // mandar tudo para o canal 2, e os dois se desfazendo
            // mutuamente enquanto ele tentava trabalhar.
            if let Some(ms) = crate::hidpp::arrived_ms_ago() {
                if ms < SETTLE {
                    continue;
                }
            }

            // Mesmo lock das trocas: nunca competir com a borda nem com a API.
            let acted = {
                let _g = focus::switch_lock();
                reconcile(&snap)
            };
            if acted.is_empty() {
                continue;
            }

            for (_, r) in &acted {
                say!("follow: {}", r.texto());
            }

            // Disjuntor: o reconciliador se desliga em vez de insistir.
            //
            // Nao e cautela abstrata -- foi medido. Sete pedidos em dois
            // minutos, nenhum com efeito, e o usuario sem conseguir usar
            // o teclado. Desligar e perder a reconciliacao automatica;
            // insistir e perder a maquina.
            match avaliar(&acted, &mut falhas) {
                Acao::Nada => continue,
                Acao::Esperar => std::thread::sleep(AFTER_ACTION),
                Acao::EsperarMais => std::thread::sleep(APOS_FALHA),
                Acao::Desligar { dev, tentativas } => {
                    gripe!(
                        "follow: {tentativas} tentativas sem efeito em '{dev}' -- DESLIGANDO \
                         o reconciliador. Resolva o motivo e religue em Disposicao."
                    );
                    if let Ok(mut g) = cfg.write() {
                        g.input.follow.enabled = false;
                    }
                    if let Err(e) = crate::config::save_follow(false, None) {
                        gripe!("follow: nao consegui gravar o desligamento: {e}");
                    }
                    falhas.clear();
                    anunciado = false;
                }
            }
        }
    });
}

#[cfg(test)]
mod testes {
    use super::*;

    fn moveu(d: &str) -> (String, Resultado) {
        (d.into(), Resultado::Moveu("ok".into()))
    }
    fn sem_efeito(d: &str) -> (String, Resultado) {
        (d.into(), Resultado::SemEfeito("aceitou e nada aconteceu".into()))
    }

    #[test]
    fn sucesso_nao_conta_falha() {
        let mut f = Default::default();
        assert_eq!(avaliar(&[moveu("mouse")], &mut f), Acao::Esperar);
        assert!(f.is_empty());
    }

    #[test]
    fn tres_sem_efeito_desligam() {
        // O caso real: sete pedidos aceitos, nenhum com efeito. Na
        // terceira tentativa o reconciliador tem de se desligar, nao
        // continuar pedindo.
        let mut f = Default::default();
        assert_eq!(avaliar(&[sem_efeito("mouse")], &mut f), Acao::EsperarMais);
        assert_eq!(avaliar(&[sem_efeito("mouse")], &mut f), Acao::EsperarMais);
        assert_eq!(
            avaliar(&[sem_efeito("mouse")], &mut f),
            Acao::Desligar { dev: "mouse".into(), tentativas: 3 }
        );
    }

    #[test]
    fn sucesso_no_meio_zera_a_contagem() {
        // Senao duas falhas espacadas por horas, com sucesso entre elas,
        // desligariam o reconciliador por motivo nenhum.
        let mut f = Default::default();
        avaliar(&[sem_efeito("mouse")], &mut f);
        avaliar(&[sem_efeito("mouse")], &mut f);
        avaliar(&[moveu("mouse")], &mut f);
        assert_eq!(avaliar(&[sem_efeito("mouse")], &mut f), Acao::EsperarMais);
    }

    #[test]
    fn a_contagem_e_por_dispositivo() {
        // Um dispositivo problematico nao condena o outro: o sucesso de
        // um nao perdoa o outro, e a falha de um nao acusa o outro.
        let mut f = Default::default();
        avaliar(&[sem_efeito("mouse"), moveu("teclado")], &mut f);
        avaliar(&[sem_efeito("mouse"), moveu("teclado")], &mut f);
        assert_eq!(
            avaliar(&[sem_efeito("mouse"), moveu("teclado")], &mut f),
            Acao::Desligar { dev: "mouse".into(), tentativas: 3 }
        );
    }

    #[test]
    fn passada_vazia_nao_mexe_em_nada() {
        let mut f = std::collections::BTreeMap::from([("mouse".to_string(), 2u32)]);
        assert_eq!(avaliar(&[], &mut f), Acao::Nada);
        assert_eq!(f.get("mouse"), Some(&2), "nao pode perdoar sozinho");
    }
}
