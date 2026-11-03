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
const PULLBACK: f64 = 24.0;

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

struct Session {
    conn: Connection,
    path: Path<'static>,
    /// Borda vigiada, para saber para que lado recuar o ponteiro.
    edge: String,
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
            eprintln!("border: ConnectToEIS falhou, seguindo sem ele: {e}");
            None
        }
    };

    sp.method_call::<(), _, _, _>(IFACE_SESSION, "Enable", ())
        .map_err(|e| format!("Enable: {e}"))?;

    println!(
        "border: barreira {bid} na borda {edge} {line:?}, zona {zone:?}, eis={}",
        if eis.is_some() { "sim" } else { "nao" }
    );
    Ok(Session { conn, path, edge: edge.to_string(), _eis: eis })
}

/// Sobe a vigilancia de borda numa thread. Erro aqui nao derruba o daemon:
/// o resto do logid continua util sem a deteccao.
pub fn spawn(cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || {
        let (edge, target) = {
            let Ok(g) = cfg.read() else { return };
            let b = &g.input.border;
            if !b.enabled {
                return;
            }
            match &b.target {
                Some(t) => (b.edge.clone(), t.clone()),
                None => {
                    eprintln!("border: habilitado sem `target`; nada a fazer");
                    return;
                }
            }
        };

        let sess = match open(&edge) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("border: desligado -- {e}");
                return;
            }
        };

        let last = Arc::new(RwLock::new(None::<Instant>));
        let rule = {
            let mut r = MatchRule::new_signal(IFACE_SESSION, "Activated");
            r.path = Some(sess.path.clone());
            r
        };

        let cfg2 = Arc::clone(&cfg);
        let path2 = sess.path.clone();
        let edge2 = sess.edge.clone();
        let target2 = target.clone();
        let last2 = Arc::clone(&last);

        let res = sess.conn.add_match(
            rule,
            move |(barrier_id, activation_id, cursor): (u32, u32, (f64, f64)), conn, _msg| {
                println!(
                    "border: ACTIVATED barreira={barrier_id} activation={activation_id} cursor={cursor:?}"
                );

                // Devolve o ponteiro IMEDIATAMENTE, e RECUADO da borda --
                // devolver no mesmo ponto re-dispara a barreira em loop.
                let (cx, cy) = cursor;
                let back = match edge2.as_str() {
                    "right" => (cx - PULLBACK, cy),
                    "left" => (cx + PULLBACK, cy),
                    "bottom" => (cx, cy - PULLBACK),
                    "top" => (cx, cy + PULLBACK),
                    _ => (cx, cy),
                };
                let sp = conn.with_proxy(DEST, path2.clone(), TIMEOUT);
                let mut opts: std::collections::HashMap<
                    &str,
                    dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,
                > = std::collections::HashMap::new();
                opts.insert(
                    "cursor_position",
                    dbus::arg::Variant(Box::new((back.0, back.1)) as Box<dyn dbus::arg::RefArg>),
                );
                if let Err(e) = sp.method_call::<(), _, _, _>(IFACE_SESSION, "Release", (opts,)) {
                    eprintln!("border: Release com cursor_position falhou ({e}); tentando sem");
                    let empty: std::collections::HashMap<
                        &str,
                        dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,
                    > = std::collections::HashMap::new();
                    if let Err(e2) = sp.method_call::<(), _, _, _>(IFACE_SESSION, "Release", (empty,)) {
                        eprintln!("border: Release falhou: {e2}");
                    }
                }

                // Cooldown: a acao abaixo e irreversivel.
                let skip = last2
                    .read()
                    .ok()
                    .and_then(|g| *g)
                    .is_some_and(|t| t.elapsed() < COOLDOWN);
                if skip {
                    println!("border: em cooldown, ignorando");
                    return true;
                }
                if let Ok(mut g) = last2.write() {
                    *g = Some(Instant::now());
                }

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
                    };
                    let outcome = focus::execute(&snap, &req);
                    if outcome.executed {
                        for r in &outcome.results {
                            println!("border: {r}");
                        }
                    } else {
                        let bloqueios: Vec<&str> = outcome
                            .plan
                            .checks
                            .iter()
                            .filter(|c| c.blocking && !c.ok)
                            .map(|c| c.name.as_str())
                            .collect();
                        println!(
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
            eprintln!("border: add_match falhou: {e}");
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
            eprintln!("border: match de diagnostico falhou: {e}");
        }
        sess.conn.start_receive(
            MatchRule::new(),
            Box::new(|msg, _| {
                if msg
                    .interface()
                    .is_some_and(|i| i.contains("InputCapture"))
                {
                    println!(
                        "border[sinal] {}.{}",
                        msg.interface().map(|i| i.to_string()).unwrap_or_default(),
                        msg.member().map(|m| m.to_string()).unwrap_or_default()
                    );
                }
                true
            }),
        );

        println!("border: vigiando borda '{edge}' -> {target}");
        let mut ticks: u64 = 0;
        loop {
            if let Err(e) = sess.conn.process(Duration::from_millis(500)) {
                eprintln!("border: loop do barramento morreu: {e}");
                return;
            }
            ticks += 1;
            // Prova de vida a cada ~60 s: sem isto, silencio no journal e
            // ambiguo entre "nada aconteceu" e "a thread morreu".
            if ticks % 120 == 0 {
                println!("border: vivo, {} min vigiando '{}'", ticks / 120, edge);
            }
        }
    });
}
