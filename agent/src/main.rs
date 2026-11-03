//! logid-agent -- o lado Windows.
//!
//! Spec: ../specs/005-input-switch.md
//!
//! E uma CLI, nao um servidor, de proposito: o `logid` o invoca por SSH,
//! que ja temos autenticado por chave. Zero porta nova, zero auth nova,
//! zero superficie de rede. Saida em JSON para o control plane consumir.
//!
//! A excecao e o `guard`, que precisa ficar residente -- e na sessao
//! interativa, porque GetLastInputInfo nao ve input de outra sessao.

mod hidpp;
mod input;

use hidapi::HidApi;
use serde_json::json;
use std::process::ExitCode;

fn out(v: serde_json::Value) -> ExitCode {
    println!("{v}");
    ExitCode::SUCCESS
}

fn fail(msg: impl AsRef<str>) -> ExitCode {
    println!("{}", json!({ "ok": false, "error": msg.as_ref() }));
    ExitCode::FAILURE
}

fn usage() -> ExitCode {
    eprintln!(
        "logid-agent <comando>

  devices                      interfaces HID++ e host atual de cada slot
  host-info <slot>             numero de hosts e canal atual (slot 1..6)
  set-host <slot> <host> --confirm
                               troca o canal. 1-based no CLI.
                               IRREVERSIVEL: o dispositivo sai deste
                               receptor e nao responde mais aqui.
  input-age                    ms desde o ultimo input nesta sessao
  guard <slot> <host> <segundos>
                               dead-man's switch: se nao houver input em
                               <segundos>, devolve o dispositivo para
                               <host>. Rode na sessao interativa."
    );
    ExitCode::FAILURE
}

/// Varre os slots 1..6 colhendo host info de quem responder.
fn survey(api: &HidApi) -> serde_json::Value {
    let interfaces: Vec<_> = hidpp::list(api)
        .into_iter()
        .map(|(path, pid, usage, iface)| {
            json!({ "path": path, "product_id": format!("{pid:04x}"),
                    "usage": usage, "interface": iface })
        })
        .collect();

    let link = match hidpp::Link::open(api, None) {
        Ok(l) => l,
        Err(e) => {
            return json!({ "ok": false, "error": e, "interfaces": interfaces });
        }
    };

    let mut slots = Vec::new();
    for slot in 1u8..=6 {
        match link.host_info(slot) {
            Ok(hi) => slots.push(json!({
                "slot": slot,
                "online": true,
                "num_hosts": hi.num_hosts,
                // O protocolo e 0-based; expomos 1-based para casar com o
                // que o Solaar e a config usam.
                "current_host": hi.current + 1,
            })),
            Err(e) => slots.push(json!({ "slot": slot, "online": false, "detail": e })),
        }
    }
    json!({ "ok": true, "interfaces": interfaces, "slots": slots })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        return usage();
    };

    if cmd == "input-age" {
        return match input::idle_ms() {
            Ok(ms) => out(json!({ "ok": true, "idle_ms": ms })),
            Err(e) => fail(e),
        };
    }

    let api = match HidApi::new() {
        Ok(a) => a,
        Err(e) => return fail(format!("hidapi: {e}")),
    };

    match cmd {
        "devices" => out(survey(&api)),

        // Sondagem: imprime os bytes crus para eu confirmar layout de
        // feature antes de escrever parser em cima de suposicao.
        "raw" => {
            let slot = args.get(1).and_then(|s| s.parse::<u8>().ok());
            let feat = args.get(2).and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok());
            let func = args.get(3).and_then(|s| s.parse::<u8>().ok());
            let (Some(slot), Some(feat), Some(func)) = (slot, feat, func) else {
                return fail("uso: raw <slot> <feature hex> <func> [p0 p1 p2]");
            };
            let p = |i: usize| args.get(i).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            let link = match hidpp::Link::open(&api, None) {
                Ok(l) => l,
                Err(e) => return fail(e),
            };
            match link.raw(slot, feat, func, [p(4), p(5), p(6)]) {
                Ok(r) => out(json!({ "ok": true,
                    "hex": r.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
                    "ascii": r.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect::<String>(),
                    "bytes": r })),
                Err(e) => fail(e),
            }
        }

        "host-info" => {
            let Some(slot) = args.get(1).and_then(|s| s.parse::<u8>().ok()) else {
                return fail("uso: host-info <slot 1..6>");
            };
            let link = match hidpp::Link::open(&api, None) {
                Ok(l) => l,
                Err(e) => return fail(e),
            };
            match link.host_info(slot) {
                Ok(hi) => out(json!({ "ok": true, "slot": slot,
                                      "num_hosts": hi.num_hosts,
                                      "current_host": hi.current + 1 })),
                Err(e) => fail(e),
            }
        }

        "set-host" => {
            let slot = args.get(1).and_then(|s| s.parse::<u8>().ok());
            let host = args.get(2).and_then(|s| s.parse::<u8>().ok());
            let (Some(slot), Some(host)) = (slot, host) else {
                return fail("uso: set-host <slot> <host 1-based> --confirm");
            };
            if host == 0 {
                return fail("host e 1-based");
            }
            // Mesma postura do /api/focus: acao irreversivel nao acontece
            // por acidente nem por digitacao distraida.
            if !args.iter().any(|a| a == "--confirm") {
                return fail("falta --confirm: esta acao e irreversivel \
                             (o dispositivo sai deste receptor)");
            }
            let link = match hidpp::Link::open(&api, None) {
                Ok(l) => l,
                Err(e) => return fail(e),
            };
            match link.set_host(slot, host - 1) {
                Ok(()) => out(json!({ "ok": true, "slot": slot, "host": host })),
                Err(e) => fail(e),
            }
        }

        "guard" => {
            let slot = args.get(1).and_then(|s| s.parse::<u8>().ok());
            let host = args.get(2).and_then(|s| s.parse::<u8>().ok());
            let secs = args.get(3).and_then(|s| s.parse::<u64>().ok());
            let (Some(slot), Some(host), Some(secs)) = (slot, host, secs) else {
                return fail("uso: guard <slot> <host 1-based> <segundos>");
            };
            let budget_ms = secs * 1000;
            let started = std::time::Instant::now();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(400));
                match input::idle_ms() {
                    Ok(idle) if idle < budget_ms => {
                        // Houve input: o usuario chegou, missao cumprida.
                        return out(json!({ "ok": true, "action": "none",
                                           "reason": "input detectado",
                                           "idle_ms": idle }));
                    }
                    Ok(idle) => {
                        if started.elapsed().as_millis() as u64 >= budget_ms {
                            let link = match hidpp::Link::open(&api, None) {
                                Ok(l) => l,
                                Err(e) => return fail(format!("resgate falhou ao abrir: {e}")),
                            };
                            return match link.set_host(slot, host - 1) {
                                Ok(()) => out(json!({ "ok": true, "action": "reverted",
                                                      "to_host": host, "idle_ms": idle })),
                                Err(e) => fail(format!("resgate falhou: {e}")),
                            };
                        }
                    }
                    Err(e) => return fail(e),
                }
            }
        }

        _ => usage(),
    }
}
