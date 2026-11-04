//! Deteccao de borda via org.gnome.Mutter.InputCapture.
//!
//! Spec: ../specs/005-input-switch.md#detecção-de-borda-mutter-direto-não-o-portal
//!
//! NAO usamos o portal: o `org.freedesktop.portal.InputCapture` aceita
//! CreateSession e nunca responde, porque nao consegue exibir o dialogo de
//! consentimento para um processo sem janela. O mutter expoe a mesma
//! funcionalidade de forma sincrona, sem dialogo.
//!
//! Usamos SO a deteccao: ao receber Activated devolvemos o ponteiro na hora
//! com Release. Nao capturamos nem encaminhamos input -- quem move o foco e
//! a troca de canal HID++.

use crate::config::Config;
use crate::focus::{self, FocusRequest};
use dbus::arg::OwnedFd;
use dbus::blocking::Connection;
use dbus::channel::MatchingReceiver;
use dbus::message::MatchRule;
use dbus::Path;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const DEST: &str = "org.gnome.Mutter.InputCapture";
const OBJ: &str = "/org/gnome/Mutter/InputCapture";
const IFACE: &str = "org.gnome.Mutter.InputCapture";
const IFACE_SESSION: &str = "org.gnome.Mutter.InputCapture.Session";
const CAP_POINTER: u32 = 2;
const TIMEOUT: Duration = Duration::from_secs(10);

/// Quantos pixels para dentro devolver o ponteiro.
///
/// Medido: devolver na propria borda re-dispara a barreira imediatamente --
/// chegamos a `activation=40` com o cursor parado em x=2560. O Release
/// aceita `cursor_position`, entao recuamos.
/// MEDIDO E DESCARTADO: o mutter aceita `cursor_position` no `Release` e
/// **ignora**. Em 300+ ativacoes o cursor permaneceu exatamente em
/// x=2560.0, e geramos 690 linhas de "em cooldown". Recuar o ponteiro nao
/// e uma opcao disponivel.
///
/// A estrategia que funciona e DESARMAR a barreira (`Disable`) durante o
/// cooldown e rearmar depois -- sem barreira nao ha evento, independente de
/// onde o ponteiro esteja.
const _PULLBACK_NAO_FUNCIONA: () = ();

/// Permanencia exigida na borda antes de trocar.
///
/// O mutter **nao tem** conceito de dwell: a barreira dispara no contato.
/// Aproximamos pela DENSIDADE de ativacoes: encostar e insistir re-dispara
/// varias vezes em sequencia (medimos `activation=54` com o cursor parado),
/// enquanto um raspao de passagem gera uma so.
///
/// Enquanto coletamos, o `Release` vai SEM recuo -- de proposito: e o
/// re-disparo que serve de sinal. So recuamos ao decidir.
/// Defaults; a config manda (`[input.border] hold_hits/hold_window_ms`).
/// Baixado de 3/400ms para 2/300ms: o usuario sentia delay, e um raspao
/// de passagem gera uma ativacao so -- duas ja indicam intencao.
const HOLD_WINDOW_DEFAULT: Duration = Duration::from_millis(300);
const HOLD_HITS_DEFAULT: usize = 2;

/// Janela em que uma segunda travessia e ignorada. A acao disparada e
/// IRREVERSIVEL; sem isto, um encostao duplo na borda viraria duas trocas.
/// 10 s porque a troca + verificacao por efeito leva ~5 s: com 3 s o segundo
/// evento entrava antes de o primeiro terminar.
const COOLDOWN: Duration = Duration::from_secs(10);

/// (x1, y1, x2, y2) da linha de barreira para a borda pedida.
///
/// A linha fica na FRONTEIRA da zona. Usar a ultima coluna/linha de dentro
/// (x+w-1) faz o mutter recusar com
/// "AccessDenied: Line overlaps with monitor region".
fn barrier_line(edge: &str, zone: (u32, u32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let (w, h, x, y) = zone;
    let (w, h) = (w as i32, h as i32);
    match edge {
        "right" => Some((x + w, y, x + w, y + h)),
        "left" => Some((x, y, x, y + h)),
        "top" => Some((x, y, x + w, y)),
        "bottom" => Some((x, y + h, x + w, y + h)),
        _ => None,
    }
}

/// Devolve o ponteiro ao usuario.
///
/// Sem `cursor_position`: o mutter ignora essa opcao (medido). O ponteiro
/// volta onde estava, e e por isso que o controle de churn tem de ser o
/// `Disable` da barreira, nao o recuo do cursor.
fn release(sp: &dbus::blocking::Proxy<'_, &Connection>) {
    let empty: std::collections::HashMap<&str, dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>> =
        std::collections::HashMap::new();
    if let Err(e) = sp.method_call::<(), _, _, _>(IFACE_SESSION, "Release", (empty,)) {
        gripe!("border: Release falhou: {e}");
    }
}

struct Session {
    conn: Connection,
    path: Path<'static>,
    /// Mantido vivo de proposito: fechar o fd pode derrubar a sessao.
    /// Ainda NAO foi medido se o Enable funciona sem ele -- entrou no
    /// prototipo para eliminar hipotese e ficou como ordem conhecida-boa.
    _eis: Option<OwnedFd>,
}

fn open(edge: &str) -> Result<Session, String> {
    let conn = Connection::new_session().map_err(|e| format!("barramento de sessao: {e}"))?;

    let root = conn.with_proxy(DEST, OBJ, TIMEOUT);
    let (path,): (Path,) = root
        .method_call(IFACE, "CreateSession", (CAP_POINTER,))
        .map_err(|e| format!("CreateSession: {e}"))?;
    let path = path.into_static();

    let sp = conn.with_proxy(DEST, path.clone(), TIMEOUT);

    let (serial, zones): (u32, Vec<(u32, u32, i32, i32)>) = sp
        .method_call(IFACE_SESSION, "GetZones", ())
        .map_err(|e| format!("GetZones: {e}"))?;
    let zone = *zones.first().ok_or("o compositor nao reportou nenhuma zona")?;

    let line = barrier_line(edge, zone)
        .ok_or_else(|| format!("borda invalida: '{edge}' (use left/right/top/bottom)"))?;
    let (bid,): (u32,) = sp
        .method_call(IFACE_SESSION, "AddBarrier", (serial, line))
        .map_err(|e| format!("AddBarrier em {line:?}: {e}"))?;

    let eis: Option<OwnedFd> = match sp.method_call::<(OwnedFd,), _, _, _>(
        IFACE_SESSION,
        "ConnectToEIS",
        (),
    ) {
        Ok((fd,)) => Some(fd),
        Err(e) => {
            gripe!("border: ConnectToEIS falhou, seguindo sem ele: {e}");
            None
        }
    };

    sp.method_call::<(), _, _, _>(IFACE_SESSION, "Enable", ())
        .map_err(|e| format!("Enable: {e}"))?;

    say!(
        "border: barreira {bid} na borda {edge} {line:?}, zona {zone:?}, eis={}",
        if eis.is_some() { "sim" } else { "nao" }
    );
    Ok(Session { conn, path, _eis: eis })
}

/// Sobe a vigilancia de borda numa thread. Erro aqui nao derruba o daemon:
/// o resto do logid continua util sem a deteccao.
pub fn spawn(cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || {
        let (edge, target, hold_hits, hold_window) = {
            let Ok(g) = cfg.read() else { return };
            let b = &g.input.border;
            if !b.enabled {
                return;
            }
            let hits = if b.hold_hits == 0 { HOLD_HITS_DEFAULT } else { b.hold_hits };
            let win = if b.hold_window_ms == 0 {
                HOLD_WINDOW_DEFAULT
            } else {
                Duration::from_millis(b.hold_window_ms)
            };
            match &b.target {
                Some(t) => (b.edge.clone(), t.clone(), hits, win),
                None => {
                    gripe!("border: habilitado sem `target`; nada a fazer");
                    return;
                }
            }
        };

        let sess = match open(&edge) {
            Ok(s) => s,
            Err(e) => {
                gripe!("border: desligado -- {e}");
                return;
            }
        };

        let last = Arc::new(RwLock::new(None::<Instant>));
        let hits = Arc::new(RwLock::new(Vec::<Instant>::new()));
        // Quando rearmar a barreira. O loop principal e quem chama Enable:
        // a sessao pertence a ESTA conexao D-Bus, outra nao a controla.
        let reenable = Arc::new(RwLock::new(None::<Instant>));
        let rule = {
            let mut r = MatchRule::new_signal(IFACE_SESSION, "Activated");
            r.path = Some(sess.path.clone());
            r
        };

        let cfg2 = Arc::clone(&cfg);
        let path2 = sess.path.clone();
        let target2 = target.clone();
        let last2 = Arc::clone(&last);
        let hits2 = Arc::clone(&hits);
        let reenable2 = Arc::clone(&reenable);

        let res = sess.conn.add_match(
            rule,
            move |(barrier_id, activation_id, cursor): (u32, u32, (f64, f64)), conn, _msg| {
                say!(
                    "border: ACTIVATED barreira={barrier_id} activation={activation_id} cursor={cursor:?}"
                );

                let sp = conn.with_proxy(DEST, path2.clone(), TIMEOUT);
                release(&sp);

                // Guarda: com a barreira desarmada nao deveria chegar evento.
                if last2.read().ok().and_then(|g| *g).is_some_and(|t| t.elapsed() < COOLDOWN) {
                    return true;
                }

                // Permanencia por densidade de ativacoes.
                let hits = {
                    let Ok(mut v) = hits2.write() else { return true };
                    let now = Instant::now();
                    v.retain(|t: &Instant| now.duration_since(*t) < hold_window);
                    v.push(now);
                    v.len()
                };
                if hits < hold_hits {
                    say!("border: permanencia {hits}/{hold_hits}, aguardando");
                    return true;
                }

                // Decidido. DESARMA a barreira: e o unico jeito de parar o
                // churn, ja que o mutter ignora o recuo do cursor.
                if let Err(e) = sp.method_call::<(), _, _, _>(IFACE_SESSION, "Disable", ()) {
                    gripe!("border: Disable falhou: {e}");
                }
                if let Ok(mut v) = hits2.write() {
                    v.clear();
                }
                let now = Instant::now();
                if let Ok(mut g) = last2.write() {
                    *g = Some(now);
                }
                if let Ok(mut g) = reenable2.write() {
                    *g = Some(now + COOLDOWN);
                }
                say!("border: barreira desarmada; trocando e rearmando em {}s", COOLDOWN.as_secs());

                // A troca leva segundos (solaar + verificacao por efeito).
                // Rodar aqui bloqueava o loop do D-Bus: medido, 35 s sem
                // processar sinal nenhum. Vai para uma thread.
                let Ok(snap) = cfg2.read().map(|g| g.clone()) else { return true };
                let target3 = target2.clone();
                std::thread::spawn(move || {
                    // O pre-flight completo roda aqui dentro: config ruim nao
                    // pode te deixar sem teclado so porque o mouse encostou
                    // na borda.
                    let req = FocusRequest {
                        host: target3,
                        route: None,
                        confirm: true,
                        // Travessia de borda nao espera SSH: le o cache de
                        // pares. Era ~2,2 s dos 3,9 s de uma troca.
                        fast: true,
                    };
                    let outcome = focus::execute(&snap, &req);
                    if outcome.executed {
                        for r in &outcome.results {
                            say!("border: {r}");
                        }
                    } else {
                        let bloqueios: Vec<&str> = outcome
                            .plan
                            .checks
                            .iter()
                            .filter(|c| c.blocking && !c.ok)
                            .map(|c| c.name.as_str())
                            .collect();
                        say!(
                            "border: NAO trocou -- {} (bloqueios: {})",
                            outcome.error.unwrap_or_else(|| "plano nao pronto".into()),
                            bloqueios.join(", ")
                        );
                    }
                });
                true
            },
        );
        if let Err(e) = res {
            gripe!("border: add_match falhou: {e}");
            return;
        }

        // Diagnostico: um match no barramento para TODOS os sinais da
        // interface da sessao, mais um receptor local que loga o que chegar.
        // Serve para distinguir "o mutter nao emitiu" de "nosso matcher nao
        // casou" -- ambiguidade que custou um teste inconclusivo.
        if let Err(e) = sess
            .conn
            .add_match_no_cb(&format!("type='signal',interface='{IFACE_SESSION}'"))
        {
            gripe!("border: match de diagnostico falhou: {e}");
        }
        sess.conn.start_receive(
            MatchRule::new(),
            Box::new(|msg, _| {
                if msg
                    .interface()
                    .is_some_and(|i| i.contains("InputCapture"))
                {
                    say!(
                        "border[sinal] {}.{}",
                        msg.interface().map(|i| i.to_string()).unwrap_or_default(),
                        msg.member().map(|m| m.to_string()).unwrap_or_default()
                    );
                }
                true
            }),
        );

        say!(
            "border: vigiando borda '{edge}' -> {target} \
             (permanencia {hold_hits} em {}ms)",
            hold_window.as_millis()
        );
        let mut ticks: u64 = 0;
        loop {
            if let Err(e) = sess.conn.process(Duration::from_millis(500)) {
                gripe!("border: loop do barramento morreu: {e}");
                return;
            }
            ticks += 1;

            // Rearma a barreira quando o cooldown vence. Tem de ser aqui:
            // a sessao pertence a esta conexao.
            let due = reenable.read().ok().and_then(|g| *g).is_some_and(|t| Instant::now() >= t);
            if due {
                let sp = sess.conn.with_proxy(DEST, sess.path.clone(), TIMEOUT);
                match sp.method_call::<(), _, _, _>(IFACE_SESSION, "Enable", ()) {
                    Ok(()) => say!("border: barreira rearmada"),
                    Err(e) => gripe!("border: Enable no rearme falhou: {e}"),
                }
                if let Ok(mut g) = reenable.write() {
                    *g = None;
                }
            }

            // Prova de vida a cada ~60 s: sem isto, silencio no journal e
            // ambiguo entre "nada aconteceu" e "a thread morreu".
            if ticks % 120 == 0 {
                say!("border: vivo, {} min vigiando '{}'", ticks / 120, edge);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zona de 2560x1440 na origem, como a desta maquina.
    const Z: (u32, u32, i32, i32) = (2560, 1440, 0, 0);

    #[test]
    fn barreira_fica_na_fronteira_nao_dentro() {
        // x+w, nao x+w-1: com 2559 o mutter recusa com
        // "AccessDenied: Line overlaps with monitor region".
        assert_eq!(barrier_line("right", Z), Some((2560, 0, 2560, 1440)));
        assert_eq!(barrier_line("left", Z), Some((0, 0, 0, 1440)));
        assert_eq!(barrier_line("top", Z), Some((0, 0, 2560, 0)));
        assert_eq!(barrier_line("bottom", Z), Some((0, 1440, 2560, 1440)));
    }

    #[test]
    fn barreira_respeita_offset_da_zona() {
        // Monitor secundario comecando em x=2560.
        let z = (1920, 1080, 2560, 0);
        assert_eq!(barrier_line("right", z), Some((4480, 0, 4480, 1080)));
        assert_eq!(barrier_line("left", z), Some((2560, 0, 2560, 1080)));
    }

    #[test]
    fn borda_invalida_e_recusada() {
        assert_eq!(barrier_line("diagonal", Z), None);
        assert_eq!(barrier_line("", Z), None);
    }
}
