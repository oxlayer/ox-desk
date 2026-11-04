//! KVM por LAN: manda teclado e mouse para outra maquina pela rede.
//!
//! Spec: ../../specs/005-input-switch.md
//!
//! A alternativa ao HID++ 0x1814. Aquele manda o PROPRIO dispositivo
//! trocar de canal Easy-Switch, o que exige um receptor pareado na
//! maquina de destino. Com um dongle so -- que foi onde paramos depois
//! que dois dongles perto se provaram instaveis -- apertar "mandar para
//! windows-host" fazia o teclado procurar um receptor que nao existe la,
//! nao achar, e voltar. Era o comportamento correto do protocolo
//! resolvendo o problema errado.
//!
//! Aqui o dispositivo nao sai do lugar: capturamos localmente
//! (`EVIOCGRAB`) e injetamos na outra ponta pelo mesmo stream de input
//! que a tela remota ja usa.
//!
//! ## Um processo filho, nao uma thread
//!
//! O `logistream-kvm` roda como processo separado, e isso e desenho,
//! nao preguica: **matar o processo solta o grab**, porque o kernel
//! solta quando o fd fecha. Se o KVM fosse uma thread do `logid` e ela
//! travasse, o teclado ficaria agarrado e a unica saida seria matar o
//! agente inteiro -- que e a coisa que voce usa para consertar as
//! outras. Processo filho faz da desistencia uma operacao de uma linha.

use std::process::{Child, Stdio};
use std::sync::Mutex;

use crate::config::Config;

/// O filho vivo e para quem ele esta mandando.
static ATUAL: Mutex<Option<(String, Child)>> = Mutex::new(None);

/// Para quem o input esta indo agora, ou `None` se esta aqui.
pub fn alvo() -> Option<String> {
    let mut g = ATUAL.lock().ok()?;
    // Colher o filho antes de responder: se ele morreu (socorro pelos
    // dois Ctrl, alvo caiu, rede sumiu), o input ja voltou para esta
    // maquina e dizer o contrario deixaria a bandeja mentindo.
    if let Some((_, c)) = g.as_mut() {
        if matches!(c.try_wait(), Ok(Some(_))) {
            *g = None;
        }
    }
    g.as_ref().map(|(n, _)| n.clone())
}

/// Traz o input de volta para esta maquina. Idempotente.
pub fn soltar() -> Result<(), String> {
    let mut g = ATUAL.lock().map_err(|_| "estado do KVM travado")?;
    if let Some((nome, mut c)) = g.take() {
        let _ = c.kill();
        let _ = c.wait();
        say!("kvm: input de volta (estava em {nome})");
    }
    Ok(())
}

/// Manda o input para `host`. Solta o alvo anterior, se houver.
///
/// A ordem importa: soltamos o anterior ANTES de tentar o novo. Dois
/// `EVIOCGRAB` no mesmo dispositivo nao coexistem, e o segundo falharia
/// com "Device or resource busy" -- deixando o input preso no destino
/// velho e a mensagem de erro falando de outra coisa.
pub fn mandar(cfg: &Config, host: &str) -> Result<String, String> {
    let h = cfg
        .hosts
        .iter()
        .find(|x| x.name == host)
        .ok_or_else(|| format!("host '{host}' nao esta na config"))?;
    // Mandar para a maquina local = trazer de volta. Mesmo teste de
    // "e aqui?" que o focus.rs usa, e nao so a flag `local`: ela e
    // opcional na config e nem todo host a declara, entao confiar so
    // nela faria "Trazer para ubuntu-host" tentar abrir um KVM
    // apontando para nos mesmos.
    let aqui = h.local.unwrap_or(false)
        || matches!(h.addr.as_str(), "127.0.0.1" | "localhost" | "::1");
    if aqui {
        return soltar().map(|()| "input local".to_string());
    }
    let sc = h
        .screen
        .as_ref()
        .ok_or_else(|| format!("'{host}' nao tem `[hosts.screen]`: sem porta para mandar input"))?;

    soltar()?;

    // Levantar o host da outra ponta primeiro. E ele quem recebe o
    // stream de input; sem ele de pe o `logistream-kvm` recusa conectar
    // -- e recusar e o comportamento certo, porque agarrar o teclado
    // sem destino deixaria a pessoa sem teclado E sem para onde ir.
    crate::screen::acordar_host(h, sc.port)
        .map_err(|e| format!("nao consegui levantar o host de {host}: {e}"))?;

    let destino = format!("{}:{}", h.addr, sc.port);
    let filho = crate::sys::child("logistream-kvm")
        .arg(&destino)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("nao consegui executar logistream-kvm: {e}"))?;

    match ATUAL.lock() {
        Ok(mut g) => *g = Some((host.to_string(), filho)),
        Err(_) => return Err("estado do KVM travado".into()),
    }
    say!("kvm: input indo para {host} ({destino})");
    Ok(destino)
}
