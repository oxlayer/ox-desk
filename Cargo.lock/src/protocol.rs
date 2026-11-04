//! O protocolo entre os pares: uma linha de JSON por comando.
//!
//! Quem fala: `logid-agent` no Windows e o `logid` em qualquer SO, cada um
//! invocado por SSH pelo par (ver `logid/src/agent.rs`). Quem le: o
//! `agent.rs` do outro lado, que procura a primeira linha comecando com
//! `{`.
//!
//! ## Por que mora na crate compartilhada
//!
//! Isto era `survey()` dentro do `agent/src/main.rs`. Quando o `logid`
//! passou a precisar responder os mesmos comandos -- para o Windows poder
//! sondar o Ubuntu como o Ubuntu ja sondava o Windows -- a escolha era
//! copiar ou compartilhar. Duas copias do **contrato entre as maquinas**
//! divergiriam, e a primeira divergencia apareceria como "agente nao
//! respondeu JSON" num log, longe da causa.
//!
//! Aqui, os dois binarios emitem o mesmo formato por construcao.

use crate::{endpoints, Receiver};
use hidapi::HidApi;
use serde_json::{json, Value};

/// Resultado de um comando do protocolo.
pub enum Handled {
    /// Comando conhecido; este e o JSON a imprimir.
    Json(Value),
    /// Nao e um comando do protocolo -- quem chamou trata (o agente tem
    /// `watch-edge` e `guard`; o logid, servir HTTP).
    Unknown,
}

/// Executa um comando do protocolo, se for um deles.
///
/// `args` e a linha de comando **sem** o nome do programa.
pub fn run(api: &HidApi, args: &[String]) -> Handled {
    let Some(cmd) = args.first().map(String::as_str) else {
        return Handled::Unknown;
    };
    match cmd {
        "devices" => Handled::Json(devices(api)),
        "host-info" => {
            let Some(slot) = args.get(1).and_then(|s| s.parse::<u8>().ok()) else {
                return Handled::Json(fail("uso: host-info <slot 1..6>"));
            };
            Handled::Json(host_info(api, slot))
        }
        "set-host" => {
            let slot = args.get(1).and_then(|s| s.parse::<u8>().ok());
            let host = args.get(2).and_then(|s| s.parse::<u8>().ok());
            let (Some(slot), Some(host)) = (slot, host) else {
                return Handled::Json(fail("uso: set-host <slot> <host 1-based> --confirm"));
            };
            let confirmed = args.iter().any(|a| a == "--confirm");
            Handled::Json(set_host(api, slot, host, confirmed))
        }
        _ => Handled::Unknown,
    }
}

pub fn fail(msg: impl Into<String>) -> Value {
    json!({ "ok": false, "error": msg.into() })
}

/// Inventario dos slots do receptor local.
pub fn devices(api: &HidApi) -> Value {
    let interfaces: Vec<_> = endpoints(api)
        .into_iter()
        .map(|(path, pid, usage_page, usage)| {
            json!({ "path": path, "product_id": format!("{pid:04x}"),
                    "usage_page": format!("{usage_page:#06x}"), "usage": usage })
        })
        .collect();

    let link = match Receiver::open(api) {
        Ok(l) => l,
        Err(e) => return json!({ "ok": false, "error": e, "interfaces": interfaces }),
    };

    // Caminho eficiente: ping de 250 ms primeiro, e host_info (mais caro)
    // so para quem respondeu. Antes chamavamos host_info nos 6 slots --
    // medido, 6,4 s por probe.
    // Com nomes: o reconciliador casa dispositivo por NOME, nao por canal
    // -- canal e ambiguo quando mouse e teclado declaram o mesmo.
    let slots: Vec<_> = link
        .devices(6, true, false)
        .into_iter()
        .map(|d| {
            if d.online {
                json!({
                    "slot": d.slot,
                    "name": d.name,
                    "online": true,
                    "num_hosts": d.host.map(|h| h.num_hosts),
                    // Ja 1-based: o crate converte do protocolo.
                    "current_host": d.host.map(|h| h.current),
                    "ping_ms": d.reply_ms,
                })
            } else {
                json!({ "slot": d.slot, "online": false })
            }
        })
        .collect();
    json!({ "ok": true, "interfaces": interfaces, "slots": slots })
}

pub fn host_info(api: &HidApi, slot: u8) -> Value {
    let link = match Receiver::open(api) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    match link.host_info(slot) {
        Ok(hi) => json!({ "ok": true, "slot": slot,
                          "num_hosts": hi.num_hosts, "current_host": hi.current }),
        Err(e) => fail(e),
    }
}

pub fn set_host(api: &HidApi, slot: u8, host: u8, confirmed: bool) -> Value {
    if host == 0 {
        return fail("host e 1-based");
    }
    // Mesma postura do /api/focus: acao irreversivel nao acontece por
    // acidente nem por digitacao distraida.
    if !confirmed {
        return fail(
            "falta --confirm: esta acao e irreversivel \
             (o dispositivo sai deste receptor)",
        );
    }
    let link = match Receiver::open(api) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    match link.set_host(slot, host) {
        Ok(()) => json!({ "ok": true, "slot": slot, "host": host }),
        Err(e) => fail(e),
    }
}
