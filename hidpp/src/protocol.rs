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
        "hosts" => {
            let Some(slot) = args.get(1).and_then(|s| s.parse::<u8>().ok()) else {
                return Handled::Json(fail("uso: hosts <slot 1..6>"));
            };
            Handled::Json(hosts(api, slot))
        }
        _ => Handled::Unknown,
    }
}

/// Os hosts de um dispositivo, LIDOS DELE (0x1815 HOSTS_INFO).
///
/// Existe porque passei uma manha deduzindo para onde o canal 2 aponta a
/// partir do que os RECEPTORES mostram, e errei duas vezes: o receptor
/// nao sabe nomear dispositivo ausente, e slot vazio sai igual a slot
/// pareado-mas-fora. A config tambem nao serve de prova -- ela e o que
/// alguem DIGITOU. Quem sabe e o dispositivo.
///
/// O layout saiu de resposta real, nao de suposicao (ver `Receiver::raw`
/// e o historico em specs/005):
///
/// ```text
/// func 1 (getHostInfo),  param [i]:     i, ?, bus, ?, tam_nome, max
/// func 3 (getHostName),  param [i,off]: i, off, bytes do nome...
/// ```
///
/// O `bus` veio 0x01 nos hosts por receptor Unifying e 0x04 no host por
/// Bluetooth -- foi o que distinguiu, nos tres hosts do MX Keys,
/// `ubuntu-host` e `windows-host` (Unifying) do MacBook (Bluetooth).
///
/// Nao e probe: o `0x1815` e opcional e o MX Anywhere 3 nao o implementa,
/// entao a resposta diz isso em vez de inventar.
pub fn hosts(api: &HidApi, slot: u8) -> Value {
    let link = match Receiver::open(api) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    const F: u16 = 0x1815;
    const INFO: u8 = 1;
    const NOME: u8 = 3;
    /// Bytes de nome por resposta, depois dos dois de cabecalho.
    const POR_PEDACO: usize = 14;

    let mut lista = Vec::new();
    for i in 0u8..8 {
        // Fim da lista e erro, nao sentinela: o dispositivo recusa indice
        // fora de faixa. Para no primeiro que recusar.
        let info = match link.raw(slot, F, INFO, &[i, 0, 0]) {
            Ok(r) if r.len() >= 6 => r,
            Ok(_) => break,
            Err(e) if i == 0 => return fail(format!("0x1815 em slot {slot}: {e}")),
            Err(_) => break,
        };
        let tam = info[4] as usize;
        let bus = info[2];

        let mut nome = Vec::new();
        let mut off = 0usize;
        while nome.len() < tam && off < 64 {
            match link.raw(slot, F, NOME, &[i, off as u8, 0]) {
                Ok(r) if r.len() > 2 => nome.extend_from_slice(&r[2..]),
                _ => break,
            }
            off += POR_PEDACO;
        }
        nome.truncate(tam);
        // O nome vem em UTF-8 (o do Mac traz a apostrofe tipografica em
        // tres bytes), entao nao da para tratar byte a byte como ASCII.
        let nome = String::from_utf8_lossy(&nome).trim_end_matches('\0').to_string();

        lista.push(json!({
            "host": i + 1,
            "nome": nome,
            "bus": match bus { 0x01 => "unifying", 0x04 => "bluetooth", _ => "?" },
            "bus_byte": bus,
        }));
    }
    json!({ "ok": true, "slot": slot, "hosts": lista })
}

pub fn fail(msg: impl Into<String>) -> Value {
    json!({ "ok": false, "error": msg.into() })
}

/// Nomes vistos por slot, em disco.
///
/// ## Por que existe
///
/// Ler o nome exige o dispositivo RESPONDER, entao um slot pareado cujo
/// dispositivo esta em outra maquina sai do inventario igual a um slot
/// vazio: `{"slot": 1, "online": false}`. Isso nao e detalhe -- foi o que
/// me fez afirmar, errado, que o teclado nao estava pareado no receptor
/// do Windows, e perder uma hora investigando um problema de pareamento
/// que nao existia. O dispositivo estava pareado, so estava fora.
///
/// O lado Linux ja guardava nomes dentro do `logid`; o agente nao tinha
/// nada. Agora mora aqui, no crate que os dois usam, porque a pergunta
/// ("vazio ou ausente?") e a mesma nos dois.
fn caminho_cache() -> Option<std::path::PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)?.join("logid")
    } else {
        match std::env::var_os("XDG_CACHE_HOME") {
            Some(v) => std::path::PathBuf::from(v),
            None => std::path::PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
        }
        .join("logi")
    };
    Some(base.join("slot-names.json"))
}

fn nomes_lidos() -> std::collections::BTreeMap<String, String> {
    caminho_cache()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Grava o que foi visto agora, preservando o resto.
///
/// Falha em silencio de proposito: cache e conveniencia, e um inventario
/// que recusa responder porque nao conseguiu escrever em disco seria pior
/// que um inventario sem o nome de um slot ausente.
fn nomes_gravar(vistos: &std::collections::BTreeMap<String, String>) {
    let Some(p) = caminho_cache() else { return };
    let mut atual = nomes_lidos();
    atual.extend(vistos.clone());
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(t) = serde_json::to_string(&atual) {
        let _ = std::fs::write(p, t);
    }
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
    let conhecidos = nomes_lidos();
    let mut vistos = std::collections::BTreeMap::new();
    let slots: Vec<_> = link
        .devices(6, true, false)
        .into_iter()
        .map(|d| {
            if d.online {
                if let Some(n) = d.name.clone() {
                    vistos.insert(d.slot.to_string(), n);
                }
                json!({
                    "slot": d.slot,
                    "name": d.name,
                    "online": true,
                    "paired": true,
                    "num_hosts": d.host.map(|h| h.num_hosts),
                    // Ja 1-based: o crate converte do protocolo.
                    "current_host": d.host.map(|h| h.current),
                    "ping_ms": d.reply_ms,
                })
            } else {
                // Nome do cache: separa "slot vazio" de "pareado, mas o
                // dispositivo esta em outra maquina". Sem isto os dois
                // casos sao o mesmo JSON, e quem le conclui errado.
                let lembrado = conhecidos.get(&d.slot.to_string());
                json!({
                    "slot": d.slot,
                    "name": lembrado,
                    "online": false,
                    "paired": lembrado.is_some(),
                })
            }
        })
        .collect();
    nomes_gravar(&vistos);
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
