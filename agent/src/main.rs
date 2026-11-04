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

use winput as input;

use hidpp::hidapi::HidApi;
use serde_json::json;
use std::process::ExitCode;

/// Log em arquivo para os modos residentes.
///
/// Lancado por atalho do Startup, stdout e stderr vao para o nada -- e
/// foi exatamente por isso que passei a diagnosticar o lado Windows no
/// escuro. O lado Linux tem journal; este e o equivalente.
fn logline(msg: &str) {
    use std::io::Write;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{ts} {msg}\n");
    eprint!("{line}");
    let Some(dir) = std::env::var_os("LOCALAPPDATA") else { return };
    let path = std::path::Path::new(&dir).join("logid").join("watch-edge.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

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
  watch-edge <edge> <slots> <host> [hold_ms]
                               vigia a borda e manda os dispositivos para
                               <host> ao encostar. Ex: watch-edge left 1,2 1
                               Roda na sessao interativa.
  guard <slot> <host> <segundos>
                               dead-man's switch: se nao houver input em
                               <segundos>, devolve o dispositivo para
                               <host>. Rode na sessao interativa."
    );
    ExitCode::FAILURE
}

/// Varre os slots 1..6 colhendo host info de quem responder.
fn survey(api: &HidApi) -> serde_json::Value {
    let interfaces: Vec<_> = hidpp::endpoints(api)
        .into_iter()
        .map(|(path, pid, usage_page, usage)| {
            json!({ "path": path, "product_id": format!("{pid:04x}"),
                    "usage_page": format!("{usage_page:#06x}"), "usage": usage })
        })
        .collect();

    let link = match hidpp::Receiver::open(api) {
        Ok(l) => l,
        Err(e) => {
            return json!({ "ok": false, "error": e, "interfaces": interfaces });
        }
    };

    // Usa o caminho eficiente do crate: ping de 250 ms primeiro, e
    // host_info (mais caro) so para quem respondeu. Antes chamavamos
    // host_info nos 6 slots -- medido, 6,4 s por probe.
    // Com nomes: o reconciliador do logid casa dispositivo por NOME, nao
    // por canal -- canal e ambiguo quando mouse e teclado declaram o mesmo.
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
            let link = match hidpp::Receiver::open(&api) {
                Ok(l) => l,
                Err(e) => return fail(e),
            };
            match link.raw(slot, feat, func, &[p(4), p(5), p(6)]) {
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
            let link = match hidpp::Receiver::open(&api) {
                Ok(l) => l,
                Err(e) => return fail(e),
            };
            match link.host_info(slot) {
                Ok(hi) => out(json!({ "ok": true, "slot": slot,
                                      "num_hosts": hi.num_hosts,
                                      "current_host": hi.current })),
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
            let link = match hidpp::Receiver::open(&api) {
                Ok(l) => l,
                Err(e) => return fail(e),
            };
            match link.set_host(slot, host) {
                Ok(()) => out(json!({ "ok": true, "slot": slot, "host": host })),
                Err(e) => fail(e),
            }
        }

        "watch-edge" => {
            let edge = args.get(1).cloned().unwrap_or_default();
            let slots: Vec<u8> = args
                .get(2)
                .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
                .unwrap_or_default();
            let host = args.get(3).and_then(|s| s.parse::<u8>().ok());
            let hold_ms: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(250);
            let (Some(host), false) = (host, slots.is_empty()) else {
                return fail("uso: watch-edge <edge> <slot,slot> <host 1-based> [hold_ms]");
            };
            if host == 0 {
                return fail("host e 1-based");
            }
            if !matches!(edge.as_str(), "left" | "right" | "top" | "bottom") {
                return fail("edge: left | right | top | bottom");
            }

            if !input::in_console_session() {
                return fail(
                    "watch-edge precisa da sessao do console: GetCursorPos e \
                     GetLastInputInfo sao por sessao. Lancado assim ele nao \
                     veria input e ainda competiria pelo receptor. Use o \
                     atalho em shell:startup.",
                );
            }
            if !input::claim_single_instance("logid-agent-watch-edge") {
                return fail(
                    "ja existe um watch-edge rodando -- tres instancias \
                     competindo pelo mesmo receptor foi problema real",
                );
            }

            let (vx, vy, vw, vh) = input::virtual_screen();
            logline(&format!(
                "watch-edge START borda='{edge}' area=({vx},{vy} {vw}x{vh}) \
                 slots={slots:?} -> host {host} hold={hold_ms}ms"
            ));

            let mut touching_since: Option<std::time::Instant> = None;
            let mut cooldown_until: Option<std::time::Instant> = None;

            // Dead-man's switch. A spec 005 o exige e eu nunca o havia
            // ativado, porque descobri que ele NAO pode ser lancado por
            // SSH: GetLastInputInfo nao ve a sessao do usuario de fora
            // dela. Aqui dentro funciona -- este processo ja e residente
            // na sessao e ja faz polling.
            //
            // Logica: quando os dispositivos CHEGAM nesta maquina, damos
            // uma janela. Se nenhum input aparecer nela, devolvemos -- quem
            // os mandou nao pode desfazer, so quem os detem.
            let guard_secs: u64 = std::env::var("LOGID_GUARD_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(25);
            let mut presente_antes = false;
            let mut chegou_em: Option<std::time::Instant> = None;
            let mut idle_na_chegada: u64 = 0;
            // O loop gira a cada 30 ms para o cursor responder rapido, mas
            // a checagem de presenca e HID++: a 33 Hz seria martelar o
            // radio sem motivo. Uma vez por segundo basta -- a janela do
            // guard e de dezenas de segundos.
            let mut ultima_checagem = std::time::Instant::now()
                - std::time::Duration::from_secs(2);
            loop {
                std::thread::sleep(std::time::Duration::from_millis(30));

                if let Some(t) = cooldown_until {
                    if std::time::Instant::now() < t {
                        continue;
                    }
                    cooldown_until = None;
                }

                // --- dead-man's switch: detecta chegada e vigia o input
                if ultima_checagem.elapsed() >= std::time::Duration::from_secs(1) {
                    ultima_checagem = std::time::Instant::now();
                if let Ok(link) = hidpp::Receiver::open(&api) {
                    let presente = slots.iter().any(|s| link.present(*s, 2));
                    if presente && !presente_antes {
                        idle_na_chegada = input::idle_ms().unwrap_or(0);
                        chegou_em = Some(std::time::Instant::now());
                        logline(&format!(
                            "dispositivos CHEGARAM -- vigiando input por {guard_secs}s \
                             (idle na chegada: {idle_na_chegada}ms)"
                        ));
                    } else if !presente && presente_antes {
                        chegou_em = None;
                        logline("dispositivos SAIRAM -- vigilancia encerrada");
                    }
                    presente_antes = presente;

                    if let Some(t0) = chegou_em {
                        match input::idle_ms() {
                            // Input novo: o idle caiu abaixo do tempo desde
                            // a chegada, logo houve atividade depois dela.
                            Ok(idle) if idle < t0.elapsed().as_millis() as u64 => {
                                logline("input detectado -- dead-man's switch desarmado");
                                chegou_em = None;
                            }
                            Ok(idle) if t0.elapsed().as_secs() >= guard_secs => {
                                logline(&format!(
                                    "SEM INPUT em {guard_secs}s (idle {idle}ms) -- \
                                     devolvendo para o host {host}"
                                ));
                                for slot in &slots {
                                    if link.ping(*slot).is_err() {
                                        continue;
                                    }
                                    match link.set_host(*slot, host) {
                                        Ok(()) => logline(&format!(
                                            "resgate: slot {slot} -> host {host}"
                                        )),
                                        Err(e) => logline(&format!(
                                            "resgate do slot {slot} FALHOU: {e}"
                                        )),
                                    }
                                }
                                chegou_em = None;
                                cooldown_until = Some(
                                    std::time::Instant::now()
                                        + std::time::Duration::from_secs(15),
                                );
                                continue;
                            }
                            _ => {}
                        }
                    }
                }
                }

                let Some(p) = input::cursor() else { continue };
                if !input::at_edge(&edge, p) {
                    touching_since = None;
                    continue;
                }

                // Exige permanencia: evita troca por raspao na borda.
                let since = *touching_since.get_or_insert_with(std::time::Instant::now);
                if (since.elapsed().as_millis() as u64) < hold_ms {
                    continue;
                }
                touching_since = None;

                let link = match hidpp::Receiver::open(&api) {
                    Ok(l) => l,
                    Err(e) => {
                        logline(&format!("sem HID++ agora: {e}"));
                        cooldown_until =
                            Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                        continue;
                    }
                };

                // Presenca ANTES de tentar. Sem isto, encostar na borda com
                // os dispositivos em outra maquina reportava
                // "sem resposta para feature 0x00 func 0x00 ... switch_failed"
                // -- que e ausencia, nao falha. Tambem cobre numeracao de
                // slot divergente: slot e por receptor e nao coincide entre
                // maquinas.
                // `present` com retry, nao `host_info`: ver a doc dele no
                // crate. Era esta linha que fazia a borda "nao funcionar".
                let presentes: Vec<u8> = slots
                    .iter()
                    .copied()
                    .filter(|s| link.present(*s, 3))
                    .collect();
                if presentes.is_empty() {
                    logline(&format!(
                        "BORDA TOCADA em ({},{}) mas nenhum dos slots {slots:?} esta \
                         neste receptor -- nada a enviar",
                        p.x, p.y
                    ));
                    cooldown_until =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(10));
                    continue;
                }
                if presentes.len() < slots.len() {
                    logline(&format!(
                        "so {presentes:?} de {slots:?} estao aqui -- enviando esses; \
                         os outros ficam onde estao (ESTADO DIVIDIDO)"
                    ));
                }

                logline(&format!(
                    "BORDA TOCADA em ({},{}) -- enviando {presentes:?} para o host {host}",
                    p.x, p.y
                ));
                for slot in &presentes {
                    match link.set_host(*slot, host) {
                        Ok(()) => logline(&format!("slot {slot} -> host {host} enviado")),
                        Err(e) => logline(&format!("slot {slot} FALHOU: {e}")),
                    }
                }
                // Os dispositivos acabaram de sair: nao ha input aqui para
                // disparar de novo, mas se voltarem queremos vigiar outra vez.
                cooldown_until =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(10));
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
                            let link = match hidpp::Receiver::open(&api) {
                                Ok(l) => l,
                                Err(e) => return fail(format!("resgate falhou ao abrir: {e}")),
                            };
                            return match link.set_host(slot, host) {
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
