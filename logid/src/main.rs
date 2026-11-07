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
mod changelog;
#[cfg(target_os = "linux")]
mod border;
#[cfg(windows)]
mod border_win;
mod config;
mod files;
mod focus;
mod follow;
mod hidpp;
mod clip;
mod clip_arq;
mod kvm;
mod logs;
mod peers;
mod probe;
mod procs;
mod remote;
mod screen;
mod setup;
mod setup_comum;
#[cfg(target_os = "linux")]
mod setup_linux;
#[cfg(target_os = "macos")]
mod setup_mac;
#[cfg(windows)]
mod setup_win;
mod sys;
#[cfg(target_os = "linux")]
mod tray;

use config::Config;
use probe::Snapshot;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tiny_http::{Header, Request, Response, Server};

const PAGE: &str = include_str!("../static/index.html");
const SETUP: &str = include_str!("../static/setup.html");

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

/// Subcomandos antes de servir.
///
/// O `logid` tambem e o "agente" do proprio host: o par o invoca por SSH
/// com `devices` / `host-info` / `set-host` e le uma linha de JSON. Sem
/// isto, o Windows nao conseguia sondar o Ubuntu como o Ubuntu ja sondava
/// o Windows -- e, sem essa sondagem, `focus::plan` BLOQUEIA a troca,
/// porque sem alguem do outro lado capaz de devolver o dispositivo a
/// origem nao pode desfazer (specs/005).
///
/// O formato vem de `hidpp::protocol`, compartilhado com o `logid-agent`:
/// e o contrato entre as maquinas, e duas copias divergiriam.
fn run_cli(args: &[String]) -> Option<std::process::ExitCode> {
    use std::process::ExitCode;
    match args.first().map(String::as_str) {
        None => None,
        Some("serve") => None,
        // Recebe o clipboard de outra maquina, por SSH. Entrega ao
        // daemon local -- ver `clip::cli_set` para o porque de nao
        // escrever direto.
        Some("kvm-soltar") => {
            let porta = Config::load().map(|c| c.port).unwrap_or(8787);
            Some(crate::kvm::cli_soltar(porta))
        }
        Some("clip-set") => {
            let porta = Config::load().map(|c| c.port).unwrap_or(8787);
            Some(crate::clip::cli_set(porta))
        }
        #[cfg(target_os = "linux")]
        Some("clip-sonda") => Some(crate::clip_arq::cli_sonda()),
        // Recebe arquivos de outra maquina. `clip-arq-set` le um tar do
        // stdin; os outros dois servem ao caminho do rsync, que copia
        // antes e avisa depois.
        Some("clip-arq-set") => {
            let porta = Config::load().map(|c| c.port).unwrap_or(8787);
            Some(crate::clip_arq::cli_set(porta))
        }
        Some("clip-arq-dir") => Some(crate::clip_arq::cli_dir()),
        Some("clip-arq-prontos") => {
            let porta = Config::load().map(|c| c.port).unwrap_or(8787);
            match args.get(1) {
                Some(d) => Some(crate::clip_arq::cli_prontos(porta, d)),
                None => {
                    eprintln!("clip-arq-prontos: informe a pasta");
                    Some(ExitCode::FAILURE)
                }
            }
        }
        // Mesma FORMA do `/healthz` de propósito. Quem pergunta "o daemon
        // no ar é o binário que está no disco?" compara duas respostas
        // com um parser só, em vez de inventar um segundo jeito de dizer
        // a mesma coisa. Ver `setup_comum::daemon_do_disco`.
        Some("versao") | Some("--versao") => {
            println!(
                "{{\"build\":\"{}\",\"n\":{}}}",
                env!("LOGI_BUILD"),
                env!("LOGI_BUILD_N").parse::<u64>().unwrap_or(0)
            );
            Some(ExitCode::SUCCESS)
        }
        Some("-h") | Some("--help") | Some("help") => {
            // stderr: stdout e reservado para o JSON do protocolo.
            eprintln!(
                "logid [comando]

  (sem comando)  sobe o daemon e serve a API em 127.0.0.1
  serve          idem, explicito
  versao         o carimbo deste binario, na forma do /healthz

  kvm-soltar     devolve o input desta maquina para ela mesma
                 (usado por um par que quer o input de volta)
  clip-set       le o stdin e poe no clipboard desta maquina
                 (usado pelos pares por SSH)

  devices        inventario dos slots deste receptor (JSON)
  host-info <slot>
                 numero de hosts e canal atual (JSON)
  set-host <slot> <host> --confirm
                 troca o canal. 1-based. IRREVERSIVEL: o dispositivo
                 sai deste receptor e nao responde mais aqui.

Os tres ultimos existem para o PAR nos sondar por SSH; a saida e uma
linha de JSON. Ver specs/002-ssh.md."
            );
            Some(ExitCode::SUCCESS)
        }
        Some(_) => {
            // `::hidpp` e nao `hidpp`: este crate tem um modulo local com
            // o mesmo nome (o adaptador em hidpp.rs), que sombreia a crate
            // compartilhada aqui no raiz.
            let api = match ::hidpp::hidapi::HidApi::new() {
                Ok(a) => a,
                Err(e) => {
                    println!("{}", ::hidpp::protocol::fail(format!("hidapi: {e}")));
                    return Some(ExitCode::FAILURE);
                }
            };
            match ::hidpp::protocol::run(&api, args) {
                ::hidpp::protocol::Handled::Json(v) => {
                    let ok = v.get("ok").and_then(|b| b.as_bool()) == Some(true);
                    println!("{v}");
                    Some(if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE })
                }
                ::hidpp::protocol::Handled::Unknown => {
                    println!(
                        "{}",
                        ::hidpp::protocol::fail(format!(
                            "comando desconhecido: {:?} -- use --help",
                            args[0]
                        ))
                    );
                    Some(ExitCode::FAILURE)
                }
            }
        }
    }
}

fn main() -> std::process::ExitCode {
    // Antes de qualquer coisa: se vier comando, responde e sai. Nao
    // carrega config, nao abre porta -- o par so quer uma linha de JSON.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = run_cli(&args) {
        return code;
    }

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
    // Um KVM que sobreviveu ao reinicio continua valendo: adotar, nao
    // matar. Matar arrancava o input de quem estava usando a outra
    // maquina toda vez que um deploy reiniciava o daemon.
    if let Ok(g) = cfg.read() {
        kvm::adotar(&g);
    }

    // Clipboard compartilhado. No Linux uma thread com sessao D-Bus
    // propria observa e publica; nos outros, amostragem.
    #[cfg(target_os = "linux")]
    {
        let canal = clip::linux::spawn(Arc::clone(&cfg));
        clip::guardar_canal(canal);
    }
    #[cfg(not(target_os = "linux"))]
    clip::vigiar(Arc::clone(&cfg));
    // Bandeja nativa. Nao e fatal se nao subir: sem extensao de
    // AppIndicator (ou sem bandeja nenhuma) o icone nao aparece e o
    // daemon segue servindo painel e API. Derrubar o logid porque nao
    // ha onde desenhar um icone seria trocar inconveniencia por queda.
    #[cfg(target_os = "linux")]
    if let Some(h) = tray::iniciar(Arc::clone(&cfg)) {
        tray::vigiar(h, Arc::clone(&cfg));
    }

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
    // So chegamos aqui se todas as threads morreram: o servidor caiu.
    std::process::ExitCode::FAILURE
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
    // Modo LAN: captura aqui e manda pela rede. Nao passa pelo
    // planejador do HID++ porque nao ha nada a planejar -- nenhum
    // dispositivo troca de canal, nenhum receptor precisa existir do
    // outro lado, e nao ha estado parcial possivel (ou o filho subiu
    // com os dois dispositivos, ou nao subiu).
    //
    // E **nao exige `confirm`**. O dry-run existe porque o ChangeHost e
    // irreversivel da origem: depois dele este receptor nao alcanca
    // mais o dispositivo. O LAN se desfaz com os dois Ctrl ou mandando
    // para a maquina local, entao pedir confirmacao seria cerimonia sem
    // risco por tras.
    if snap.input.mode == "lan" {
        let resposta = match kvm::mandar(&snap, &fr.host) {
            Ok(destino) => serde_json::json!({
                "plan": { "host": fr.host, "route": "lan" },
                "executed": true,
                "results": [format!("input indo para {} ({destino})", fr.host)],
            }),
            Err(e) => serde_json::json!({
                "plan": { "host": fr.host, "route": "lan" },
                "executed": false,
                "results": [],
                "error": e,
            }),
        };
        let code = if resposta.get("error").is_some() { 409 } else { 200 };
        return respond(
            req,
            Response::from_string(resposta.to_string())
                .with_status_code(code)
                .with_header(json_header())
                .with_header(nostore()),
        );
    }

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

/// POST /api/clipboard -- texto cru no corpo, vai para o clipboard local.
///
/// Sem JSON de proposito: o corpo E o texto. Embrulhar clipboard em
/// JSON obrigaria a escapar o que a pessoa copiou, e clipboard carrega
/// exatamente o tipo de coisa que quebra escape -- aspas, barras,
/// quebras de linha, bytes estranhos.
fn post_clipboard(mut req: Request) {
    let mut texto = String::new();
    if req.as_reader().read_to_string(&mut texto).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    if texto.len() > clip::LIMITE {
        return err_json(req, 413, "texto acima do limite");
    }
    match clip::aplicar_local(&texto) {
        Ok(()) => ok_json(req, "{\"ok\":true}".into()),
        Err(e) => err_json(req, 500, &e),
    }
}

/// POST /api/clipboard-arquivos {"paths":[...]} -- arquivos recebidos.
///
/// JSON aqui, ao contrario do clipboard de texto, porque o corpo e uma
/// LISTA: separar caminhos por quebra de linha falharia no primeiro
/// arquivo com quebra de linha no nome, que e legal no Linux.
fn post_clipboard_arquivos(mut req: Request) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let caminhos: Vec<String> = v
        .get("paths")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if caminhos.is_empty() {
        return err_json(req, 400, "informe {\"paths\": [...]}");
    }
    match clip_arq::aplicar_local(&caminhos) {
        Ok(()) => ok_json(req, "{\"ok\":true}".into()),
        Err(e) => err_json(req, 500, &e),
    }
}

/// POST /api/clipboard-arquivos/enviar {"host":"<nome>"} -- manda a
/// selecao grande pendente para UM destino.
///
/// Existe porque acima do teto nao mandamos sozinhos: 1,4 GB para duas
/// maquinas que ninguem esta olhando gasta banda e disco alheio. A
/// bandeja chama isto com o host escolhido.
fn post_clipboard_enviar(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(host) = v.get("host").and_then(|h| h.as_str()).map(String::from) else {
        return err_json(req, 400, "informe {\"host\": \"<nome>\"}");
    };
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "estado da config corrompido"),
    };
    match clip_arq::mandar_oferta(&snap, &host) {
        Ok(d) => ok_json(req, serde_json::json!({ "ok": true, "detail": d }).to_string()),
        Err(e) => err_json(req, 409, &e),
    }
}

/// POST /api/screen {"host": "<nome>"} -- lanca o cliente de tela.
///
/// Separado do GET de proposito: o GET responde "da para clicar?" e a UI
/// desabilita o botao com o motivo escrito do lado. Um botao que abre
/// nada e pior que um botao desabilitado explicado.
fn post_transferir(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let campo = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config travada"),
    };
    match files::transferir(&snap, &campo("de"), &campo("caminho"), &campo("para"), &campo("pasta")) {
        Ok(id) => ok_json(req, serde_json::json!({ "id": id }).to_string()),
        Err(e) => err_json(req, 409, &e),
    }
}

fn post_sessao(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(host) = v.get("host").and_then(|h| h.as_str()).map(String::from) else {
        return err_json(req, 400, "informe {\"host\": \"<nome>\"}");
    };
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config travada"),
    };
    match screen::abrir_sessao(&snap, &host) {
        Ok(s) => match serde_json::to_string(&s) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serializar: {e}")),
        },
        Err(e) => err_json(req, 409, &e),
    }
}

fn post_screen(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(host) = v.get("host").and_then(|h| h.as_str()).map(String::from) else {
        return err_json(req, 400, "informe {\"host\": \"<nome>\"}");
    };
    // Omitir uma opcao vale o default: controlar sim, som e microfone
    // nao. Ligar audio sem pedir e o tipo de surpresa que assusta numa
    // reuniao.
    let d = screen::Opts::default();
    let flag = |k: &str, padrao: bool| v.get(k).and_then(|x| x.as_bool()).unwrap_or(padrao);
    let opts = screen::Opts {
        control: flag("control", d.control),
        audio: flag("audio", d.audio),
        mic: flag("mic", d.mic),
    };
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config travada"),
    };
    match screen::launch(&snap, &host, opts) {
        Ok(linha) => {
            ok_json(req, serde_json::json!({ "ok": true, "command": linha }).to_string())
        }
        Err(e) => err_json(req, 409, &e),
    }
}

/// POST /api/hidpp/pause {"minutes": N}  (0 retoma)
///
/// Pausa a SONDAGEM periodica dos dispositivos, nao a troca de canal.
/// Serve para separar "nossa leitura perturba o radio" de "o hardware
/// faz isso sozinho" sem precisar tomar o mouse de ninguem para testar:
/// pausada a leitura, se a troca ficar de pe a culpa e nossa.
fn post_hidpp_pause(mut req: Request) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let min = v.get("minutes").and_then(|x| x.as_u64()).unwrap_or(10).min(60);
    let restante = hidpp::pausar(min);
    match restante {
        Some(s) => say!("hidpp: leitura PAUSADA por {}s (diagnostico)", s),
        None => say!("hidpp: leitura retomada"),
    }
    ok_json(
        req,
        serde_json::json!({ "ok": true, "pausado_s": restante,
            "nota": if restante.is_some() {
                "a sondagem parou; trocar de canal continua funcionando"
            } else {
                "sondagem retomada"
            } })
        .to_string(),
    )
}

/// POST /api/follow {"enabled": bool, "leader": "<nome HID++>"}
///
/// Qual dispositivo manda depende de COMO a pessoa troca de maquina:
/// pela borda quem atravessa e o mouse; pelo botao Easy-Switch do
/// teclado, o lider tem de ser o teclado -- senao o reconciliador desfaz
/// a acao que ela acabou de fazer. Por isso e escolha, nao constante.
fn post_follow(mut req: Request, cfg: &RwLock<Config>, cache: &Mutex<Cache>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let atual = cfg.read().map(|g| g.input.follow.enabled).unwrap_or(false);
    let on = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(atual);
    let leader = v.get("leader").and_then(|x| x.as_str());
    if let Err(e) = config::save_follow(on, leader) {
        return err_json(req, 500, &e);
    }
    match Config::load() {
        Ok(novo) => {
            let lider = novo.input.follow.leader.clone().unwrap_or_default();
            if let Ok(mut g) = cfg.write() {
                *g = novo;
            }
            if let Ok(mut c) = cache.lock() {
                c.at = None;
            }
            say!("follow: {} com lider '{lider}'", if on { "LIGADO" } else { "desligado" });
            ok_json(
                req,
                serde_json::json!({ "ok": true, "enabled": on, "leader": lider,
                    "nota": if on { "ligado agora; o laco le o estado a cada passada" } else { "desligado agora -- a proxima passada nao age" } })
                .to_string(),
            )
        }
        Err(e) => err_json(req, 500, &format!("gravou mas nao recarregou: {e}")),
    }
}

/// POST /api/border {"enabled": bool} -- liga/desliga o KVM por borda.
fn post_border(mut req: Request, cfg: &RwLock<Config>, cache: &Mutex<Cache>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(on) = v.get("enabled").and_then(|x| x.as_bool()) else {
        return err_json(req, 400, "informe {\"enabled\": true|false}");
    };
    if let Err(e) = config::save_border_enabled(on) {
        return err_json(req, 500, &e);
    }
    match Config::load() {
        Ok(novo) => {
            if let Ok(mut g) = cfg.write() {
                *g = novo;
            }
            if let Ok(mut c) = cache.lock() {
                c.at = None;
            }
            say!("border: KVM por borda {} pela GUI", if on { "LIGADO" } else { "DESLIGADO" });
            ok_json(
                req,
                serde_json::json!({ "ok": true, "enabled": on,
                    "nota": if on {
                        "ligada; se o daemon subiu sem borda, reinicie para criar as barreiras"
                    } else {
                        "desligada agora -- a proxima encostada nao troca mais"
                    } })
                .to_string(),
            )
        }
        Err(e) => err_json(req, 500, &format!("gravou mas nao recarregou: {e}")),
    }
}

/// POST /api/layout {"posicoes": {"<host>": {"col": N, "row": M}}}
///
/// Grava por edicao cirurgica do TOML, nao por reserializacao: os
/// comentarios da config sao o registro de por que cada escolha foi
/// feita, e nao se perde isso para salvar duas coordenadas.
fn post_layout(mut req: Request, cfg: &RwLock<Config>, cache: &Mutex<Cache>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(obj) = v.get("posicoes").and_then(|p| p.as_object()) else {
        return err_json(req, 400, "informe {\"posicoes\": {\"<host>\": {\"col\":N,\"row\":M}}}");
    };
    let mut pos = std::collections::BTreeMap::new();
    for (nome, c) in obj {
        let (Some(col), Some(row)) = (
            c.get("col").and_then(|x| x.as_i64()),
            c.get("row").and_then(|x| x.as_i64()),
        ) else {
            return err_json(req, 400, &format!("posicao invalida para '{nome}'"));
        };
        pos.insert(nome.clone(), config::Cell { col: col as i32, row: row as i32 });
    }
    if let Err(e) = config::save_layout(&pos) {
        return err_json(req, 500, &e);
    }
    // Recarrega do disco: o que vale e o arquivo, e assim a resposta
    // reflete o que ficou gravado, nao o que pedimos.
    match Config::load() {
        Ok(novo) => {
            let bordas: Vec<_> = novo
                .border_edges_full()
                .into_iter()
                .map(|(e, t, ok)| serde_json::json!({ "edge": e, "target": t, "ativa": ok }))
                .collect();
            if let Ok(mut g) = cfg.write() {
                *g = novo;
            }
            if let Ok(mut c) = cache.lock() {
                c.at = None;
            }
            say!("layout: grade salva; bordas agora {}", bordas.len());
            ok_json(
                req,
                serde_json::json!({ "ok": true, "edges": bordas,
                    "nota": "a grade vale agora; barreiras novas exigem reiniciar o daemon" })
                .to_string(),
            )
        }
        Err(e) => err_json(req, 500, &format!("gravou mas nao recarregou: {e}")),
    }
}

/// POST /api/processes/kill {"host","pid","force"?,"confirm":true}
///
/// `confirm` obrigatorio pelo mesmo motivo do `/api/focus`: matar
/// processo na maquina de alguem nao pode sair de um clique distraido, e
/// nao tem desfazer.
fn post_kill(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let (Some(host), Some(pid)) = (
        v.get("host").and_then(|h| h.as_str()).map(String::from),
        v.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32),
    ) else {
        return err_json(req, 400, "informe {\"host\": \"<nome>\", \"pid\": <n>}");
    };
    let force = v.get("force").and_then(|f| f.as_bool()).unwrap_or(false);
    let confirm = v.get("confirm").and_then(|c| c.as_bool()).unwrap_or(false);
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config travada"),
    };
    match procs::kill(&snap, &host, pid, force, confirm) {
        Ok(detalhe) => {
            ok_json(req, serde_json::json!({ "ok": true, "detail": detalhe }).to_string())
        }
        Err(e) => err_json(req, 409, &e),
    }
}

/// GET /api/setup -- o que falta nesta maquina.
///
/// Mede sob demanda, e NAO entra no ciclo de refresh do painel: o passo
/// `pares` abre SSH para cada host, e um painel que repete isso a cada
/// 10 s seria uma tempestade de conexoes para responder uma pergunta que
/// muda uma vez por mes. A pagina mostra a hora da medicao e um botao.
fn get_setup(req: Request, cfg: &RwLock<Config>) {
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config travada"),
    };
    let passos = setup::catalogo_local();
    let medidas = setup::medir(&snap);
    let resumo = setup::resumo(&passos, &medidas);

    // Junta catalogo e medida num objeto so: a pagina nao deveria ter de
    // casar duas listas por id para desenhar uma linha.
    let linhas: Vec<serde_json::Value> = passos
        .iter()
        .map(|p| {
            let m = medidas.iter().find(|m| m.id == p.id);
            serde_json::json!({
                "id": p.id,
                "titulo": p.titulo,
                "porque": p.porque,
                "arrumo": p.arrumo,
                "essencial": p.essencial,
                "depende": p.depende,
                "estado": m.map(|m| m.estado).unwrap_or(setup::Estado::Erro),
                "detalhe": m.map(|m| m.detalhe.clone()).unwrap_or_else(|| "nao medido".into()),
                "esperando": setup::bloqueado_por(p, &medidas),
            })
        })
        .collect();

    let eu = snap.hosts.iter().find(|h| h.is_local());
    let corpo = serde_json::json!({
        "sistema": setup::Sistema::desta_maquina(),
        "maquina": eu.map(|h| h.name.clone()),
        "papel": eu.map(|h| h.role.clone()),
        "build": env!("LOGI_BUILD"),
        "medido": sys::hhmmss(),
        "passos": linhas,
        "resumo": resumo,
    });
    match serde_json::to_string(&corpo) {
        Ok(b) => ok_json(req, b),
        Err(e) => err_json(req, 500, &format!("serializar: {e}")),
    }
}

/// POST /api/setup/arrumar {"id":"<passo>"}
///
/// Devolve 409, nao 500, quando o reparo nao resolve: nao e defeito do
/// servidor, e o caso normal de "abri a janela, agora e com voce". A
/// pagina mostra a mensagem como instrucao, e nao como erro.
fn post_setup_arrumar(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "corpo ilegivel");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(id) = v.get("id").and_then(|i| i.as_str()) else {
        return err_json(req, 400, "informe {\"id\": \"<passo>\"}");
    };
    // So ids do catalogo DESTA maquina. Sem isto, um id qualquer cairia
    // direto no `match` do modulo de plataforma -- que hoje recusa, mas
    // nada garantiria isso no proximo passo adicionado.
    if !setup::catalogo_local().iter().any(|p| p.id == id) {
        return err_json(req, 404, &format!("'{id}' nao e um passo desta maquina"));
    }
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config travada"),
    };
    say!("setup: arrumando '{id}'");
    match setup::arrumar(id, &snap) {
        Ok(detalhe) => {
            say!("setup: '{id}' ok -- {detalhe}");
            ok_json(req, serde_json::json!({"ok": true, "detalhe": detalhe}).to_string())
        }
        Err(e) => {
            say!("setup: '{id}' nao resolveu -- {e}");
            err_json(req, 409, &e)
        }
    }
}

fn handle(req: Request, cfg: &RwLock<Config>, cache: &Mutex<Cache>) {
    let path = req.url().split('?').next().unwrap_or("").to_string();
    let method = req.method().as_str().to_string();

    if path == "/api/config" && method == "PATCH" {
        return patch_config(req, cfg, cache);
    }
    if path == "/api/kvm/soltar" && method == "POST" {
        return match kvm::soltar() {
            Ok(()) => ok_json(req, "{\"ok\":true}".into()),
            Err(e) => err_json(req, 500, &e),
        };
    }
    if path == "/api/setup/arrumar" && method == "POST" {
        return post_setup_arrumar(req, cfg);
    }
    if path == "/api/setup" && method == "GET" {
        return get_setup(req, cfg);
    }
    if path == "/api/clipboard" && method == "POST" {
        return post_clipboard(req);
    }
    if path == "/api/clipboard-arquivos" && method == "POST" {
        return post_clipboard_arquivos(req);
    }
    if path == "/api/clipboard-arquivos/enviar" && method == "POST" {
        return post_clipboard_enviar(req, cfg);
    }
    if path == "/api/screen" && method == "POST" {
        return post_screen(req, cfg);
    }
    if path == "/api/sessao" && method == "POST" {
        return post_sessao(req, cfg);
    }
    if path == "/api/files/transferir" && method == "POST" {
        return post_transferir(req, cfg);
    }
    if path == "/api/processes/kill" && method == "POST" {
        return post_kill(req, cfg);
    }
    if path == "/api/layout" && method == "POST" {
        return post_layout(req, cfg, cache);
    }
    if path == "/api/border" && method == "POST" {
        return post_border(req, cfg, cache);
    }
    if path == "/api/follow" && method == "POST" {
        return post_follow(req, cfg, cache);
    }
    if path == "/api/hidpp/pause" && method == "POST" {
        return post_hidpp_pause(req);
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
        // O `build` existe para o deploy conferir que as tres maquinas
        // rodam o MESMO commit. Ja aconteceu de o Mac ficar para tras e o
        // usuario notar antes de mim.
        "/healthz" => ok_json(
            req,
            serde_json::json!({
                "ok": true,
                "build": env!("LOGI_BUILD"),
                // Incremental: diz QUAL maquina esta atras, nao so que
                // estao diferentes.
                "n": env!("LOGI_BUILD_N").parse::<u64>().unwrap_or(0),
            })
            .to_string(),
        ),

        // Para onde o input esta indo, e por qual mecanismo.
        //
        // A bandeja PRECISA disto. Ela so mostrava "Trazer para aqui"
        // quando algum dispositivo aparecia offline -- sinal que servia
        // no HID++, onde o dispositivo some do nosso receptor ao trocar
        // de canal. No modo LAN os dispositivos NUNCA saem daqui, que e
        // o ponto: continuam online e o item nunca apareceria. O
        // usuario ficaria sem caminho de volta pelo menu, dependendo de
        // lembrar dos dois Ctrl.
        "/api/kvm" => {
            let modo = match cfg.read() {
                Ok(g) => g.input.mode.clone(),
                Err(_) => return err_json(req, 500, "config travada"),
            };
            ok_json(
                req,
                serde_json::json!({ "mode": modo, "target": kvm::alvo() }).to_string(),
            )
        }

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

        "/api/files/transferencias" => match serde_json::to_string(&files::transferencias()) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serializar: {e}")),
        },

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

        "/api/changelog" => match serde_json::to_string(&changelog::entradas()) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serializar: {e}")),
        },

        "/api/layout" => {
            let snap = match cfg.read() {
                Ok(g) => g.clone(),
                Err(_) => return err_json(req, 500, "config travada"),
            };
            let hosts: Vec<_> = snap
                .hosts
                .iter()
                .map(|h| {
                    serde_json::json!({
                        "name": h.name,
                        "local": h.is_local(),
                        "os": format!("{:?}", h.os()).to_lowercase(),
                        "layout": h.layout.map(|c| serde_json::json!({"col": c.col, "row": c.row})),
                    })
                })
                .collect();
            let bordas: Vec<_> = snap
                .border_edges_full()
                .into_iter()
                .map(|(e, t, ok)| serde_json::json!({ "edge": e, "target": t, "ativa": ok }))
                .collect();
            let body = serde_json::json!({
                "hosts": hosts,
                "edges": bordas,
                "border_enabled": snap.input.border.enabled,
                "follow_enabled": snap.input.follow.enabled,
                "follow_leader": snap.input.follow.leader,
                "hidpp_pausado_s": hidpp::pausa_restante_s(),
                "devices": snap.devices.iter().map(|d| d.name.clone()).collect::<Vec<_>>(),
            });
            ok_json(req, body.to_string())
        }

        "/api/processes" => {
            let q = req.url().split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
            let snap = match cfg.read() {
                Ok(g) => g.clone(),
                Err(_) => return err_json(req, 500, "config travada"),
            };
            let host = q
                .split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| *k == "host")
                .map(|(_, v)| files::percent_decode(v))
                .unwrap_or_else(|| {
                    snap.hosts
                        .iter()
                        .find(|h| h.is_local())
                        .map(|h| h.name.clone())
                        .unwrap_or_default()
                });
            match serde_json::to_string(&procs::list(&snap, &host)) {
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

        "/setup" | "/setup.html" => respond(
            req,
            Response::from_string(SETUP).with_header(html_header()).with_header(nostore()),
        ),

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
