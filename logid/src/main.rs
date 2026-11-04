//! logid -- control plane da infra de duas maquinas na LAN.
//!
//! Spec: ../specs/001-control-plane.md
//!
//! Escuta SO em 127.0.0.1: le portas, executa SSH e (em breve) dispara
//! troca de canal HID++. E superficie de privilegio, nao um site.
//! Ver ../specs/008-security.md.

// Sem console no Windows: isto e um daemon, e no Windows ele sobe por
// tarefa agendada na sessao do usuario -- um app de console abriria uma
// janela preta a cada login e a cada vez que a GUI o subisse.
//
// Nao afeta o JSON por SSH: o subsistema decide se um console e
// ALOCADO, nao para onde o stdout vai. Quando o pai entrega um pipe (o
// caso do SSH), a saida continua chegando. O que se perde e rodar o
// binario a mao num cmd e ver texto -- e para isso existe o log em
// %LOCALAPPDATA%\logid\logid.log.
#![cfg_attr(windows, windows_subsystem = "windows")]

// Primeiro de todos: macro_rules tem escopo TEXTUAL, entao `say!` e
// `gripe!` so existem para os modulos declarados depois deste.
#[macro_use]
mod say;

mod agent;
#[cfg(target_os = "linux")]
mod border;
#[cfg(windows)]
mod border_win;
mod config;
mod files;
mod focus;
mod follow;
mod hidpp;
mod logs;
mod peers;
mod probe;
mod screen;
mod sys;

use config::Config;
use probe::Snapshot;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tiny_http::{Header, Request, Response, Server};

const PAGE: &str = include_str!("../static/index.html");

/// Cache do snapshot. Existe porque /api/status faz rede e SSH -- medido
/// em 2.09 s na versao Python, contra 0.6 ms do /healthz. Reusar e o que
/// mantem a GUI fluida.
struct Cache {
    at: Option<Instant>,
    data: Option<Arc<Snapshot>>,
}

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap()
}
fn html_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap()
}
fn nostore() -> Header {
    Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap()
}

/// Responder nunca deve derrubar a thread: cliente que desiste no meio
/// (health check com timeout curto, aba fechada) virava BrokenPipeError
/// no journal na versao Python. Aqui o erro e so ignorado.
fn respond(req: Request, resp: Response<std::io::Cursor<Vec<u8>>>) {
    let _ = req.respond(resp);
}

fn ok_json(req: Request, body: String) {
    respond(
        req,
        Response::from_string(body).with_header(json_header()).with_header(nostore()),
    );
}

fn err_json(req: Request, code: u16, msg: &str) {
    let body = serde_json::json!({ "error": msg }).to_string();
    respond(
        req,
        Response::from_string(body)
            .with_status_code(code)
            .with_header(json_header())
            .with_header(nostore()),
    );
}

fn cached(cfg: &Config, cache: &Mutex<Cache>) -> Arc<Snapshot> {
    let ttl = Duration::from_secs(cfg.refresh_seconds.saturating_sub(2).max(2));
    if let Ok(c) = cache.lock() {
        if let (Some(at), Some(data)) = (c.at, c.data.as_ref()) {
            if at.elapsed() < ttl {
                return Arc::clone(data);
            }
        }
    }
    let fresh = Arc::new(probe::snapshot(cfg));
    if let Ok(mut c) = cache.lock() {
        c.at = Some(Instant::now());
        c.data = Some(Arc::clone(&fresh));
    }
    fresh
}

fn main() {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            // Falhar barulhento: config invalida nao vira default silencioso.
            gripe!("logid: {e}");
            std::process::exit(1);
        }
    };

    let addr = format!("127.0.0.1:{}", cfg.port);
    let server = match Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            gripe!("logid: nao consegui escutar em {addr}: {e}");
            std::process::exit(1);
        }
    };
    say!("logid em http://{addr}  (config: {})", Config::path().display());

    let cfg = Arc::new(RwLock::new(cfg));
    let cache = Arc::new(Mutex::new(Cache { at: None, data: None }));

    // Vigilancia de borda numa thread propria; falha nao derruba o daemon.
    sys::spawn_border(Arc::clone(&cfg));
    // Reconciliador do "seguir o lider", em thread propria.
    follow::spawn(Arc::clone(&cfg));
    // Mantem o estado dos pares fresco, para o caminho quente nao esperar SSH.
    peers::spawn(Arc::clone(&cfg));
    // Mantem o survey local quente: a borda nao pode pagar ~360 ms.
    hidpp::spawn_refresher();

    let server = Arc::new(server);
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let server = Arc::clone(&server);
            let cfg = Arc::clone(&cfg);
            let cache = Arc::clone(&cache);
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    handle(req, &cfg, &cache);
                }
            })
        })
        .collect();

    for w in workers {
        let _ = w.join();
    }
}

fn invalidate(cache: &Mutex<Cache>) {
    if let Ok(mut c) = cache.lock() {
        c.at = None;
        c.data = None;
    }
}

/// PATCH /api/config -- merge raso no topo, valida, grava, invalida cache.
///
/// Merge raso de proposito: substituir `hosts` inteiro e explicito e
/// previsivel. Merge profundo em lista de hosts seria adivinhacao.
fn patch_config(mut req: Request, cfg: &RwLock<Config>, cache: &Mutex<Cache>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let incoming: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return err_json(req, 400, &format!("JSON invalido: {e}")),
    };
    let serde_json::Value::Object(patch) = incoming else {
        return err_json(req, 400, "esperado um objeto JSON");
    };

    let merged = {
        let guard = match cfg.read() {
            Ok(g) => g,
            Err(_) => return err_json(req, 500, "estado da config corrompido"),
        };
        let mut base = match serde_json::to_value(&*guard) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => return err_json(req, 500, "config atual nao serializa"),
        };
        for (k, v) in patch {
            if !base.contains_key(&k) {
                return err_json(req, 400, &format!("campo desconhecido: {k}"));
            }
            base.insert(k, v);
        }
        serde_json::Value::Object(base)
    };

    let next: Config = match serde_json::from_value(merged) {
        Ok(c) => c,
        Err(e) => return err_json(req, 422, &format!("config resultante invalida: {e}")),
    };
    if next.port == 0 {
        return err_json(req, 422, "port invalida");
    }
    if next.hosts.is_empty() {
        return err_json(req, 422, "precisa de ao menos um host");
    }
    // Grava antes de aplicar: se o disco recusar, nada muda em memoria.
    if let Err(e) = next.save() {
        return err_json(req, 500, &e);
    }

    let body = match serde_json::to_string(&next) {
        Ok(b) => b,
        Err(e) => return err_json(req, 500, &format!("serializar: {e}")),
    };
    match cfg.write() {
        Ok(mut g) => *g = next,
        Err(_) => return err_json(req, 500, "nao consegui aplicar"),
    }
    invalidate(cache);
    ok_json(req, body);
}

/// POST /api/focus -- passa o foco de input para outra maquina.
///
/// **Dry-run por padrao.** Sem `"confirm": true` devolve apenas o plano,
/// porque a rota fisica e irreversivel do lado de origem: depois do
/// ChangeHost este receptor nao alcanca mais o dispositivo. Ver
/// ../specs/005-input-switch.md.
fn post_focus(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let fr: focus::FocusRequest = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return err_json(req, 400, &format!("JSON invalido: {e}")),
    };
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "estado da config corrompido"),
    };
    let outcome = focus::execute(&snap, &fr);
    let code = if outcome.error.is_some() { 409 } else { 200 };
    match serde_json::to_string(&outcome) {
        Ok(b) => respond(
            req,
            Response::from_string(b)
                .with_status_code(code)
                .with_header(json_header())
                .with_header(nostore()),
        ),
        Err(e) => err_json(req, 500, &format!("serializar: {e}")),
    }
}

/// POST /api/controls/remap -- remapeia um botao.
///
/// Exige `confirm`: escrever no dispositivo muda comportamento fisico, e
/// um remapeamento errado deixa o botao inutil ate ser desfeito.
fn post_remap(mut req: Request) {
    #[derive(serde::Deserialize)]
    struct Body {
        slot: u8,
        /// "0x0053" ou decimal.
        cid: String,
        /// Destino; "0x0" ou "0" volta ao padrao.
        to: String,
        #[serde(default)]
        confirm: bool,
    }
    fn parse_id(s: &str) -> Option<u16> {
        let t = s.trim();
        if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            u16::from_str_radix(h, 16).ok()
        } else {
            t.parse().ok()
        }
    }

    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let b: Body = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return err_json(req, 400, &format!("JSON invalido: {e}")),
    };
    if !b.confirm {
        return err_json(req, 400, "falta confirm: isto muda o comportamento fisico do botao");
    }
    let (Some(cid), Some(to)) = (parse_id(&b.cid), parse_id(&b.to)) else {
        return err_json(req, 400, "cid/to invalidos (use 0x0053 ou decimal)");
    };
    match hidpp::remap(b.slot, cid, to) {
        Ok(detail) => ok_json(req, serde_json::json!({ "ok": true, "detail": detail }).to_string()),
        Err(e) => err_json(req, 409, &e),
    }
}

/// POST /api/screen {"host": "<nome>"} -- lanca o cliente de tela.
///
/// Separado do GET de proposito: o GET responde "da para clicar?" e a UI
/// desabilita o botao com o motivo escrito do lado. Um botao que abre
/// nada e pior que um botao desabilitado explicado.
fn post_screen(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let host = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("host").and_then(|h| h.as_str()).map(String::from));
    let Some(host) = host else {
        return err_json(req, 400, "informe {\"host\": \"<nome>\"}");
    };
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config travada"),
    };
    match screen::launch(&snap, &host) {
        Ok(linha) => {
            ok_json(req, serde_json::json!({ "ok": true, "command": linha }).to_string())
        }
        Err(e) => err_json(req, 409, &e),
    }
}

fn handle(req: Request, cfg: &RwLock<Config>, cache: &Mutex<Cache>) {
    let path = req.url().split('?').next().unwrap_or("").to_string();
    let method = req.method().as_str().to_string();

    if path == "/api/config" && method == "PATCH" {
        return patch_config(req, cfg, cache);
    }
    if path == "/api/screen" && method == "POST" {
        return post_screen(req, cfg);
    }
    if path == "/api/focus" && method == "POST" {
        return post_focus(req, cfg);
    }
    if path == "/api/controls/remap" && method == "POST" {
        return post_remap(req);
    }

    // Snapshot curto da config para as rotas de leitura: nao segurar o
    // lock durante rede nem durante o respond.
    let snap_cfg = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "estado da config corrompido"),
    };

    match path.as_str() {
        // Barato de proposito: NAO toca em rede. O health check do Electron
        // batia no /api/status com 800 ms de timeout e falhava sempre.
        "/healthz" => ok_json(req, r#"{"ok":true}"#.into()),

        "/api/status" | "/api/hosts" => {
            let snap = cached(&snap_cfg, cache);
            match serde_json::to_string(&*snap) {
                Ok(b) => ok_json(req, b),
                Err(e) => err_json(req, 500, &format!("serializar: {e}")),
            }
        }

        "/api/logs" => {
            // ?lines=N, limitado dentro de logs::tail.
            let n = req
                .url()
                .split_once("lines=")
                .and_then(|(_, v)| v.split('&').next())
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or_else(logs::default_lines);
            match serde_json::to_string(&logs::tail(n)) {
                Ok(b) => ok_json(req, b),
                Err(e) => err_json(req, 500, &format!("serializar: {e}")),
            }
        }

        "/api/files" => {
            // ?host=<nome>&path=<caminho>. O `path` vem percent-encoded:
            // ele carrega `/`, `\\`, `:` e espaco, e nenhum deles pode
            // ser confundido com separador da query.
            let q = req.url().split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
            let param = |k: &str| -> Option<String> {
                q.split('&')
                    .filter_map(|kv| kv.split_once('='))
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| files::percent_decode(v))
            };
            let snap = match cfg.read() {
                Ok(g) => g.clone(),
                Err(_) => return err_json(req, 500, "config travada"),
            };
            // Sem `host`: a propria maquina, que e o caso comum de abrir
            // o painel e so olhar.
            let host = param("host").unwrap_or_else(|| {
                snap.hosts
                    .iter()
                    .find(|h| h.is_local())
                    .map(|h| h.name.clone())
                    .unwrap_or_default()
            });
            let path = param("path").unwrap_or_default();
            match serde_json::to_string(&files::list(&snap, &host, &path)) {
                Ok(b) => ok_json(req, b),
                Err(e) => err_json(req, 500, &format!("serializar: {e}")),
            }
        }

        "/api/screen" => {
            let snap = match cfg.read() {
                Ok(g) => g.clone(),
                Err(_) => return err_json(req, 500, "config travada"),
            };
            match serde_json::to_string(&screen::plans(&snap)) {
                Ok(b) => ok_json(req, b),
                Err(e) => err_json(req, 500, &format!("serializar: {e}")),
            }
        }

        "/api/controls" => match serde_json::to_string(&hidpp::controls()) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serializar: {e}")),
        },

        "/api/devices" => match serde_json::to_string(&hidpp::survey()) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serializar: {e}")),
        },

        "/api/config" => match serde_json::to_string(&snap_cfg) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serializar: {e}")),
        },

        "/" | "/index.html" => {
            // Renderizado por requisicao: se refresh_seconds mudar por
            // PATCH, a pagina reflete sem reiniciar o daemon.
            let page = PAGE.replace("__REFRESH__", &snap_cfg.refresh_seconds.to_string());
            respond(
                req,
                Response::from_string(page).with_header(html_header()).with_header(nostore()),
            )
        }

        _ => err_json(req, 404, "nao encontrado"),
    }
}
