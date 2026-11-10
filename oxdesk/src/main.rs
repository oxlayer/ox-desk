//! oxdesk -- control plane for the two-machine LAN setup.
//!
//! Spec: ../specs/001-control-plane.md
//!
//! Listens ONLY on 127.0.0.1: reads ports, runs SSH and (soon) triggers
//! HID++ channel switches. It is a privileged surface, not a website.
//! See ../specs/008-security.md.

// No console on Windows: this is a daemon, and on Windows it starts from
// a scheduled task in the user's session -- a console app would open a
// black window at every login and every time the GUI started it.
//
// It does not affect the JSON over SSH: the subsystem decides whether a
// console is ALLOCATED, not where stdout goes. When the parent hands over
// a pipe (the SSH case), output still arrives. What is lost is running
// the binary by hand in a cmd and seeing text -- and for that there is
// the log at %LOCALAPPDATA%\oxdesk\oxdesk.log.
#![cfg_attr(windows, windows_subsystem = "windows")]

// First of all: macro_rules has TEXTUAL scope, so `say!` and `gripe!`
// only exist for the modules declared after this one.
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
mod clip_files;
mod kvm;
mod logs;
mod legacy;
mod pairing;
mod peers;
mod probe;
mod procs;
mod remote;
mod screen;
mod setup;
mod setup_common;
#[cfg(target_os = "linux")]
mod setup_linux;
#[cfg(target_os = "macos")]
mod setup_mac;
#[cfg(windows)]
mod setup_win;
mod sys;
mod telemetry;
#[cfg(target_os = "linux")]
mod tray;
mod wait;

use config::Config;
use probe::Snapshot;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tiny_http::{Header, Request, Response, Server};

const PAGE: &str = include_str!("../static/index.html");
const SETUP: &str = include_str!("../static/setup.html");

/// Snapshot cache. It exists because /api/status does network and SSH --
/// measured at 2.09 s in the Python version, against 0.6 ms for /healthz.
/// Reusing it is what keeps the GUI smooth.
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

/// Responding must never take the thread down: a client that gives up
/// midway (a health check with a short timeout, a closed tab) became a
/// BrokenPipeError in the journal in the Python version. Here the error
/// is simply ignored.
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

/// Subcommands before serving.
///
/// `oxdesk` is also its own host's "agent": the peer invokes it over SSH
/// with `devices` / `host-info` / `set-host` and reads one line of JSON.
/// Without this, Windows could not probe Ubuntu the way Ubuntu already
/// probed Windows -- and, without that probe, `focus::plan` BLOCKS the
/// switch, because without someone on the other side able to give the
/// device back, the origin cannot undo (specs/005).
///
/// The format comes from `hidpp::protocol`, shared with `oxdesk-agent`: it
/// is the contract between the machines, and two copies would diverge.
fn run_cli(args: &[String]) -> Option<std::process::ExitCode> {
    use std::process::ExitCode;
    match args.first().map(String::as_str) {
        None => None,
        Some("serve") => None,
        // Receives the clipboard from another machine, over SSH. Hands it
        // to the local daemon -- see `clip::cli_set` for why it does not
        // write directly.
        Some("kvm-soltar") => {
            let port = Config::load().map(|c| c.port).unwrap_or(8787);
            Some(crate::kvm::cli_release(port))
        }
        Some("clip-set") => {
            let port = Config::load().map(|c| c.port).unwrap_or(8787);
            Some(crate::clip::cli_set(port))
        }
        #[cfg(target_os = "linux")]
        Some("clip-sonda") => Some(crate::clip_files::cli_probe()),
        // Receives files from another machine. `clip-arq-set` reads a tar
        // from stdin; the other two serve the rsync path, which copies
        // first and notifies afterwards.
        Some("clip-arq-set") => {
            let port = Config::load().map(|c| c.port).unwrap_or(8787);
            Some(crate::clip_files::cli_set(port))
        }
        Some("clip-arq-dir") => Some(crate::clip_files::cli_dir()),
        Some("clip-arq-prontos") => {
            let port = Config::load().map(|c| c.port).unwrap_or(8787);
            match args.get(1) {
                Some(d) => Some(crate::clip_files::cli_ready(port, d)),
                None => {
                    eprintln!("clip-arq-prontos: give the folder");
                    Some(ExitCode::FAILURE)
                }
            }
        }
        // Same SHAPE as `/healthz` on purpose. Whoever asks "is the live
        // daemon the binary on disk?" compares two answers with a single
        // parser, instead of inventing a second way of saying the same
        // thing. See `setup_common::daemon_on_disk`.
        Some("versao") | Some("--versao") => {
            println!(
                "{{\"build\":\"{}\",\"n\":{}}}",
                env!("OXDESK_BUILD"),
                env!("OXDESK_BUILD_N").parse::<u64>().unwrap_or(0)
            );
            Some(ExitCode::SUCCESS)
        }
        Some("-h") | Some("--help") | Some("help") => {
            // stderr: stdout is reserved for the protocol JSON.
            eprintln!(
                "oxdesk [command]

  (no command)   starts the daemon and serves the API on 127.0.0.1
  serve          same, explicit
  versao         this binary's build stamp, in the /healthz shape

  kvm-soltar     gives this machine's input back to itself
                 (used by a peer that wants the input back)
  clip-set       reads stdin and puts it on this machine's clipboard
                 (used by the peers over SSH)

  devices        inventory of this receiver's slots (JSON)
  host-info <slot>
                 number of hosts and current channel (JSON)
  set-host <slot> <host> --confirm
                 switches the channel. 1-based. IRREVERSIBLE: the device
                 leaves this receiver and no longer answers here.

The last three exist so the PEER can probe us over SSH; the output is
one line of JSON. See specs/002-ssh.md."
            );
            Some(ExitCode::SUCCESS)
        }
        Some(_) => {
            // `::hidpp` and not `hidpp`: this crate has a local module with
            // the same name (the adapter in hidpp.rs), which shadows the
            // shared crate here at the root.
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
                            "unknown command: {:?} -- use --help",
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
    // Before anything else: if a command came, answer and exit. Do not
    // load the config, do not open the port -- the peer only wants one
    // line of JSON.
    // Before anything reads the config: an installation from before the
    // rename keeps its config, identity and pairing (legacy.rs).
    for line in legacy::migrate() {
        say!("legacy: {line}");
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = run_cli(&args) {
        return code;
    }

    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            // Fail loudly: an invalid config does not become a silent
            // default.
            gripe!("oxdesk: {e}");
            std::process::exit(1);
        }
    };

    let addr = format!("127.0.0.1:{}", cfg.port);
    let server = match Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            gripe!("oxdesk: could not listen on {addr}: {e}");
            std::process::exit(1);
        }
    };
    say!("oxdesk on http://{addr}  (config: {})", Config::path().display());

    let cfg = Arc::new(RwLock::new(cfg));
    let cache = Arc::new(Mutex::new(Cache { at: None, data: None }));

    // Border watch on its own thread; a failure does not take the daemon
    // down.
    sys::spawn_border(Arc::clone(&cfg));
    // "Follow the leader" reconciler, on its own thread.
    follow::spawn(Arc::clone(&cfg));
    // Keeps the peers' state fresh, so the hot path does not wait on SSH.
    peers::spawn(Arc::clone(&cfg));
    // Keeps the local survey warm: the border cannot pay ~360 ms.
    hidpp::spawn_refresher();
    // A KVM that survived the restart is still valid: adopt it, do not
    // kill it. Killing it yanked the input away from whoever was using
    // the other machine every time a deploy restarted the daemon.
    if let Ok(g) = cfg.read() {
        kvm::adopt(&g);
    }

    // Shared clipboard. On Linux a thread with its own D-Bus session
    // watches and publishes; on the others, polling.
    #[cfg(target_os = "linux")]
    {
        let channel = clip::linux::spawn(Arc::clone(&cfg));
        clip::store_channel(channel);
    }
    #[cfg(not(target_os = "linux"))]
    clip::watch(Arc::clone(&cfg));
    // Native tray. Not fatal if it does not come up: without the
    // AppIndicator extension (or without any tray) the icon does not
    // appear and the daemon keeps serving the panel and the API. Taking
    // oxdesk down because there is nowhere to draw an icon would trade an
    // inconvenience for an outage.
    //
    // It does keep asking, though. Giving up at t=0 is what left this
    // machine with no icon for half an hour -- see `wait::retry_delay`.
    #[cfg(target_os = "linux")]
    tray::start_when_drawable(Arc::clone(&cfg));

    // Spec 017: pair with the peers once at startup, so phase C can refuse
    // strangers without refusing our own machines.
    if let Ok(c) = cfg.read() {
        pairing::pair_peers(c.hosts.clone(), screen::local_host_bin());
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
    // We only get here if all threads died: the server went down.
    std::process::ExitCode::FAILURE
}

fn invalidate(cache: &Mutex<Cache>) {
    if let Ok(mut c) = cache.lock() {
        c.at = None;
        c.data = None;
    }
}

/// PATCH /api/config -- shallow top-level merge, validate, save,
/// invalidate the cache.
///
/// Shallow merge on purpose: replacing `hosts` whole is explicit and
/// predictable. A deep merge into a list of hosts would be guesswork.
fn patch_config(mut req: Request, cfg: &RwLock<Config>, cache: &Mutex<Cache>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let incoming: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return err_json(req, 400, &format!("invalid JSON: {e}")),
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
            _ => return err_json(req, 500, "the current config does not serialize"),
        };
        for (k, v) in patch {
            if !base.contains_key(&k) {
                return err_json(req, 400, &format!("unknown field: {k}"));
            }
            base.insert(k, v);
        }
        serde_json::Value::Object(base)
    };

    let next: Config = match serde_json::from_value(merged) {
        Ok(c) => c,
        Err(e) => return err_json(req, 422, &format!("the resulting config is invalid: {e}")),
    };
    if next.port == 0 {
        return err_json(req, 422, "invalid port");
    }
    if next.hosts.is_empty() {
        return err_json(req, 422, "precisa de ao menos um host");
    }
    // Save before applying: if the disk refuses, nothing changes in
    // memory.
    if let Err(e) = next.save() {
        return err_json(req, 500, &e);
    }

    let body = match serde_json::to_string(&next) {
        Ok(b) => b,
        Err(e) => return err_json(req, 500, &format!("serialize: {e}")),
    };
    match cfg.write() {
        Ok(mut g) => *g = next,
        Err(_) => return err_json(req, 500, "could not apply it"),
    }
    invalidate(cache);
    ok_json(req, body);
}

/// POST /api/focus -- hands input focus to another machine.
///
/// **Dry-run by default.** Without `"confirm": true` it returns only the
/// plan, because the physical route is irreversible on the origin side:
/// after ChangeHost this receiver no longer reaches the device. See
/// ../specs/005-input-switch.md.
fn post_focus(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let fr: focus::FocusRequest = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return err_json(req, 400, &format!("invalid JSON: {e}")),
    };
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "estado da config corrompido"),
    };
    // LAN mode: capture here and send over the network. It does not go
    // through the HID++ planner because there is nothing to plan -- no
    // device switches channel, no receiver needs to exist on the other
    // side, and there is no possible partial state (either the child
    // started with both devices, or it did not).
    //
    // And it **does not require `confirm`**. The dry-run exists because
    // ChangeHost is irreversible from the origin: after it this receiver
    // no longer reaches the device. LAN is undone with the two Ctrls or
    // by sending to the local machine, so asking for confirmation would
    // be ceremony with no risk behind it.
    if snap.input.mode == "lan" {
        let response = match kvm::send(&snap, &fr.host) {
            Ok(dest) => serde_json::json!({
                "plan": { "host": fr.host, "route": "lan" },
                "executed": true,
                "results": [format!("input going to {} ({dest})", fr.host)],
            }),
            Err(e) => serde_json::json!({
                "plan": { "host": fr.host, "route": "lan" },
                "executed": false,
                "results": [],
                "error": e,
            }),
        };
        let code = if response.get("error").is_some() { 409 } else { 200 };
        return respond(
            req,
            Response::from_string(response.to_string())
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
        Err(e) => err_json(req, 500, &format!("serialize: {e}")),
    }
}

/// POST /api/controls/remap -- remaps a button.
///
/// Requires `confirm`: writing to the device changes physical behavior,
/// and a wrong remap leaves the button useless until it is undone.
fn post_remap(mut req: Request) {
    #[derive(serde::Deserialize)]
    struct Body {
        slot: u8,
        /// "0x0053" or decimal.
        cid: String,
        /// Target; "0x0" or "0" restores the default.
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
        return err_json(req, 400, "unreadable body");
    }
    let b: Body = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return err_json(req, 400, &format!("invalid JSON: {e}")),
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

/// POST /api/clipboard -- raw text in the body, goes to the local
/// clipboard.
///
/// No JSON on purpose: the body IS the text. Wrapping the clipboard in
/// JSON would force escaping what the person copied, and the clipboard
/// carries exactly the kind of thing that breaks escaping -- quotes,
/// slashes, line breaks, odd bytes.
fn post_clipboard(mut req: Request) {
    let mut text = String::new();
    if req.as_reader().read_to_string(&mut text).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    if text.len() > clip::LIMIT {
        return err_json(req, 413, "texto acima do limite");
    }
    match clip::apply_local(&text) {
        Ok(()) => ok_json(req, "{\"ok\":true}".into()),
        Err(e) => err_json(req, 500, &e),
    }
}

/// POST /api/clipboard-files {"paths":[...]} -- received files.
///
/// JSON here, unlike the text clipboard, because the body is a LIST:
/// separating paths by line break would fail on the first file with a
/// line break in its name, which is legal on Linux.
fn post_clipboard_files(mut req: Request) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let paths: Vec<String> = v
        .get("paths")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if paths.is_empty() {
        return err_json(req, 400, "send {\"paths\": [...]}");
    }
    match clip_files::apply_local(&paths) {
        Ok(()) => ok_json(req, "{\"ok\":true}".into()),
        Err(e) => err_json(req, 500, &e),
    }
}

/// POST /api/clipboard-files/send {"host":"<name>"} -- sends the
/// pending large selection to ONE destination.
///
/// It exists because above the cap we do not send on our own: 1.4 GB to
/// two machines nobody is looking at wastes someone else's bandwidth and
/// disk. The tray calls this with the chosen host.
fn post_clipboard_send(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(host) = v.get("host").and_then(|h| h.as_str()).map(String::from) else {
        return err_json(req, 400, "send {\"host\": \"<name>\"}");
    };
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "estado da config corrompido"),
    };
    match clip_files::send_offer(&snap, &host) {
        Ok(d) => ok_json(req, serde_json::json!({ "ok": true, "detail": d }).to_string()),
        Err(e) => err_json(req, 409, &e),
    }
}

/// POST /api/screen {"host": "<name>"} -- launches the screen client.
///
/// Separate from the GET on purpose: the GET answers "can it be clicked?"
/// and the UI disables the button with the reason written beside it. A
/// button that opens nothing is worse than a disabled, explained button.
fn post_transfer(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config lock poisoned"),
    };
    match files::transfer(&snap, &field("from"), &field("path"), &field("to"), &field("folder")) {
        Ok(id) => ok_json(req, serde_json::json!({ "id": id }).to_string()),
        Err(e) => err_json(req, 409, &e),
    }
}

fn post_session(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(host) = v.get("host").and_then(|h| h.as_str()).map(String::from) else {
        return err_json(req, 400, "send {\"host\": \"<name>\"}");
    };
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config lock poisoned"),
    };
    match screen::open_session(&snap, &host) {
        Ok(s) => match serde_json::to_string(&s) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serialize: {e}")),
        },
        Err(e) => err_json(req, 409, &e),
    }
}

fn post_screen(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(host) = v.get("host").and_then(|h| h.as_str()).map(String::from) else {
        return err_json(req, 400, "send {\"host\": \"<name>\"}");
    };
    // Omitting an option means the default: control yes, sound and
    // microphone no. Turning audio on without asking is the kind of
    // surprise that scares people in a meeting.
    let d = screen::Opts::default();
    let flag = |k: &str, fallback: bool| v.get(k).and_then(|x| x.as_bool()).unwrap_or(fallback);
    let opts = screen::Opts {
        control: flag("control", d.control),
        audio: flag("audio", d.audio),
        mic: flag("mic", d.mic),
    };
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config lock poisoned"),
    };
    match screen::launch(&snap, &host, opts) {
        Ok(line) => {
            ok_json(req, serde_json::json!({ "ok": true, "command": line }).to_string())
        }
        Err(e) => err_json(req, 409, &e),
    }
}

/// POST /api/hidpp/pause {"minutes": N}  (0 resumes)
///
/// Pauses the periodic POLLING of the devices, not channel switching. It
/// serves to tell "our reading disturbs the radio" apart from "the
/// hardware does this on its own" without taking anyone's mouse away to
/// test: with reading paused, if the switch holds, the fault is ours.
fn post_hidpp_pause(mut req: Request) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let min = v.get("minutes").and_then(|x| x.as_u64()).unwrap_or(10).min(60);
    let remaining = hidpp::pause(min);
    match remaining {
        Some(s) => say!("hidpp: polling PAUSED for {}s (diagnostics)", s),
        None => say!("hidpp: polling resumed"),
    }
    ok_json(
        req,
        serde_json::json!({ "ok": true, "paused_s": remaining,
            "note": if remaining.is_some() {
                "a sondagem parou; trocar de canal continua funcionando"
            } else {
                "sondagem retomada"
            } })
        .to_string(),
    )
}

/// POST /api/follow {"enabled": bool, "leader": "<HID++ name>"}
///
/// Which device leads depends on HOW the person switches machines: over
/// the border it is the mouse that crosses; with the keyboard's
/// Easy-Switch button, the leader has to be the keyboard -- otherwise
/// the reconciler undoes the action they just took. That is why it is a
/// choice, not a constant.
fn post_follow(mut req: Request, cfg: &RwLock<Config>, cache: &Mutex<Cache>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let current = cfg.read().map(|g| g.input.follow.enabled).unwrap_or(false);
    let on = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(current);
    let leader = v.get("leader").and_then(|x| x.as_str());
    if let Err(e) = config::save_follow(on, leader) {
        return err_json(req, 500, &e);
    }
    match Config::load() {
        Ok(reloaded) => {
            let leader_name = reloaded.input.follow.leader.clone().unwrap_or_default();
            if let Ok(mut g) = cfg.write() {
                *g = reloaded;
            }
            if let Ok(mut c) = cache.lock() {
                c.at = None;
            }
            say!("follow: {} with leader '{leader_name}'", if on { "ON" } else { "off" });
            ok_json(
                req,
                serde_json::json!({ "ok": true, "enabled": on, "leader": leader_name,
                    "note": if on { "on now; the loop reads the state on every pass" } else { "off now -- the next pass does nothing" } })
                .to_string(),
            )
        }
        Err(e) => err_json(req, 500, &format!("written but not reloaded: {e}")),
    }
}

/// POST /api/border {"enabled": bool} -- turns the border KVM on/off.
fn post_border(mut req: Request, cfg: &RwLock<Config>, cache: &Mutex<Cache>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(on) = v.get("enabled").and_then(|x| x.as_bool()) else {
        return err_json(req, 400, "send {\"enabled\": true|false}");
    };
    if let Err(e) = config::save_border_enabled(on) {
        return err_json(req, 500, &e);
    }
    match Config::load() {
        Ok(reloaded) => {
            if let Ok(mut g) = cfg.write() {
                *g = reloaded;
            }
            if let Ok(mut c) = cache.lock() {
                c.at = None;
            }
            say!("border: border KVM {} from the GUI", if on { "ON" } else { "OFF" });
            ok_json(
                req,
                serde_json::json!({ "ok": true, "enabled": on,
                    "note": if on {
                        "on; if the daemon started without edge detection, restart it to create the barriers"
                    } else {
                        "off now -- the next touch of the edge will not switch"
                    } })
                .to_string(),
            )
        }
        Err(e) => err_json(req, 500, &format!("written but not reloaded: {e}")),
    }
}

/// POST /api/layout {"positions": {"<host>": {"col": N, "row": M}}}
///
/// Saves by surgical TOML editing, not by reserializing: the config's
/// comments are the record of why each choice was made, and that is not
/// lost to save two coordinates.
fn post_layout(mut req: Request, cfg: &RwLock<Config>, cache: &Mutex<Cache>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(obj) = v.get("positions").and_then(|p| p.as_object()) else {
        return err_json(req, 400, "send {\"positions\": {\"<host>\": {\"col\":N,\"row\":M}}}");
    };
    let mut pos = std::collections::BTreeMap::new();
    for (name, c) in obj {
        let (Some(col), Some(row)) = (
            c.get("col").and_then(|x| x.as_i64()),
            c.get("row").and_then(|x| x.as_i64()),
        ) else {
            return err_json(req, 400, &format!("invalid position for '{name}'"));
        };
        pos.insert(name.clone(), config::Cell { col: col as i32, row: row as i32 });
    }
    if let Err(e) = config::save_layout(&pos) {
        return err_json(req, 500, &e);
    }
    // Reload from disk: the file is what counts, and this way the
    // response reflects what was saved, not what we asked for.
    match Config::load() {
        Ok(reloaded) => {
            let edges: Vec<_> = reloaded
                .border_edges_full()
                .into_iter()
                .map(|(e, t, ok)| serde_json::json!({ "edge": e, "target": t, "active": ok }))
                .collect();
            if let Ok(mut g) = cfg.write() {
                *g = reloaded;
            }
            if let Ok(mut c) = cache.lock() {
                c.at = None;
            }
            say!("layout: grid saved; edges now {}", edges.len());
            ok_json(
                req,
                serde_json::json!({ "ok": true, "edges": edges,
                    "note": "a grade vale agora; barreiras novas exigem reiniciar o daemon" })
                .to_string(),
            )
        }
        Err(e) => err_json(req, 500, &format!("written but not reloaded: {e}")),
    }
}

/// POST /api/processes/kill {"host","pid","force"?,"confirm":true}
///
/// `confirm` is mandatory for the same reason as `/api/focus`: killing a
/// process on someone's machine cannot come from a distracted click, and
/// it has no undo.
fn post_kill(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let (Some(host), Some(pid)) = (
        v.get("host").and_then(|h| h.as_str()).map(String::from),
        v.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32),
    ) else {
        return err_json(req, 400, "send {\"host\": \"<name>\", \"pid\": <n>}");
    };
    let force = v.get("force").and_then(|f| f.as_bool()).unwrap_or(false);
    let confirm = v.get("confirm").and_then(|c| c.as_bool()).unwrap_or(false);
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config lock poisoned"),
    };
    match procs::kill(&snap, &host, pid, force, confirm) {
        Ok(detail) => {
            ok_json(req, serde_json::json!({ "ok": true, "detail": detail }).to_string())
        }
        Err(e) => err_json(req, 409, &e),
    }
}

/// GET /api/setup -- what this machine still lacks.
///
/// Measures on demand, and does NOT join the panel's refresh cycle: the
/// `peers` step opens SSH to each host, and a panel repeating that every
/// 10 s would be a storm of connections to answer a question that changes
/// once a month. The page shows the measurement time and a button.
fn get_setup(req: Request, cfg: &RwLock<Config>) {
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config lock poisoned"),
    };
    let steps = setup::local_catalog();
    let checks = setup::measure(&snap);
    let summary = setup::summarize(&steps, &checks);

    // Joins catalog and check in a single object: the page should not
    // have to match two lists by id to draw one row.
    let rows: Vec<serde_json::Value> = steps
        .iter()
        .map(|p| {
            let m = checks.iter().find(|m| m.id == p.id);
            serde_json::json!({
                "id": p.id,
                "title": p.title,
                "why": p.why,
                "fix": p.fix,
                "essential": p.essential,
                "depends_on": p.depends_on,
                "state": m.map(|m| m.state).unwrap_or(setup::State::Error),
                "detail": m.map(|m| m.detail.clone()).unwrap_or_else(|| "not measured".into()),
                "waiting_on": setup::blocked_by(p, &checks),
            })
        })
        .collect();

    let me = snap.hosts.iter().find(|h| h.is_local());
    let body = serde_json::json!({
        "system": setup::System::this_machine(),
        "machine": me.map(|h| h.name.clone()),
        "role": me.map(|h| h.role.clone()),
        "build": env!("OXDESK_BUILD"),
        "measured": sys::hhmmss(),
        "steps": rows,
        "summary": summary,
    });
    match serde_json::to_string(&body) {
        Ok(b) => ok_json(req, b),
        Err(e) => err_json(req, 500, &format!("serialize: {e}")),
    }
}

/// POST /api/setup/fix {"id":"<step>"}
///
/// Returns 409, not 500, when the repair does not solve it: it is not a
/// server defect, it is the normal "I opened the window, now it is up to
/// you" case. The page shows the message as an instruction, not as an
/// error.
fn post_setup_repair(mut req: Request, cfg: &RwLock<Config>) {
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return err_json(req, 400, "unreadable body");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let Some(id) = v.get("id").and_then(|i| i.as_str()) else {
        return err_json(req, 400, "send {\"id\": \"<step>\"}");
    };
    // Only ids from THIS machine's catalog. Without this, any id would
    // fall straight into the platform module's `match` -- which refuses
    // today, but nothing would guarantee that for the next step added.
    if !setup::local_catalog().iter().any(|p| p.id == id) {
        return err_json(req, 404, &format!("'{id}' is not a step on this machine"));
    }
    let snap = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "config lock poisoned"),
    };
    say!("setup: repairing '{id}'");
    match setup::repair(id, &snap) {
        Ok(detail) => {
            say!("setup: '{id}' ok -- {detail}");
            ok_json(req, serde_json::json!({"ok": true, "detail": detail}).to_string())
        }
        Err(e) => {
            say!("setup: '{id}' not solved -- {e}");
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
    if path == "/api/kvm/release" && method == "POST" {
        return match kvm::release() {
            Ok(()) => ok_json(req, "{\"ok\":true}".into()),
            Err(e) => err_json(req, 500, &e),
        };
    }
    if path == "/api/setup/fix" && method == "POST" {
        return post_setup_repair(req, cfg);
    }
    if path == "/api/setup" && method == "GET" {
        return get_setup(req, cfg);
    }
    if path == "/api/clipboard" && method == "POST" {
        return post_clipboard(req);
    }
    if path == "/api/clipboard-files" && method == "POST" {
        return post_clipboard_files(req);
    }
    if path == "/api/clipboard-files/send" && method == "POST" {
        return post_clipboard_send(req, cfg);
    }
    if path == "/api/screen" && method == "POST" {
        return post_screen(req, cfg);
    }
    if path == "/api/session" && method == "POST" {
        return post_session(req, cfg);
    }
    if path == "/api/files/transfer" && method == "POST" {
        return post_transfer(req, cfg);
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

    // Short config snapshot for the read routes: do not hold the lock
    // during network calls or during respond.
    let snap_cfg = match cfg.read() {
        Ok(g) => g.clone(),
        Err(_) => return err_json(req, 500, "estado da config corrompido"),
    };

    match path.as_str() {
        // Cheap on purpose: does NOT touch the network. The Electron health
        // check hit /api/status with an 800 ms timeout and always failed.
        // `build` exists so the deploy can check that the three machines
        // run the SAME commit. The Mac has already fallen behind and the
        // user noticed before I did.
        "/healthz" => ok_json(
            req,
            serde_json::json!({
                "ok": true,
                "build": env!("OXDESK_BUILD"),
                // Incremental: says WHICH machine is behind, not just that
                // they differ.
                "n": env!("OXDESK_BUILD_N").parse::<u64>().unwrap_or(0),
            })
            .to_string(),
        ),

        // Where input is going, and through which mechanism.
        //
        // The tray NEEDS this. It only showed "Bring it here" when some
        // device appeared offline -- a signal that worked for HID++, where
        // the device vanishes from our receiver when it switches channel.
        // In LAN mode the devices NEVER leave here, which is the point:
        // they stay online and the item would never appear. The user
        // would have no way back through the menu, depending on
        // remembering the two Ctrls.
        "/api/kvm" => {
            let mode = match cfg.read() {
                Ok(g) => g.input.mode.clone(),
                Err(_) => return err_json(req, 500, "config lock poisoned"),
            };
            ok_json(
                req,
                serde_json::json!({ "mode": mode, "target": kvm::target() }).to_string(),
            )
        }

        "/api/telemetry" => {
            ok_json(req, telemetry::summary_json(50));
            return;
        }
        "/api/status" | "/api/hosts" => {
            let snap = cached(&snap_cfg, cache);
            match serde_json::to_string(&*snap) {
                Ok(b) => ok_json(req, b),
                Err(e) => err_json(req, 500, &format!("serialize: {e}")),
            }
        }

        "/api/logs" => {
            // ?lines=N, capped inside logs::tail.
            let n = req
                .url()
                .split_once("lines=")
                .and_then(|(_, v)| v.split('&').next())
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or_else(logs::default_lines);
            match serde_json::to_string(&logs::tail(n)) {
                Ok(b) => ok_json(req, b),
                Err(e) => err_json(req, 500, &format!("serialize: {e}")),
            }
        }

        "/api/files" => {
            // ?host=<name>&path=<path>. `path` comes percent-encoded: it
            // carries `/`, `\\`, `:` and spaces, and none of them can be
            // mistaken for a query separator.
            let q = req.url().split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
            let param = |k: &str| -> Option<String> {
                q.split('&')
                    .filter_map(|kv| kv.split_once('='))
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| files::percent_decode(v))
            };
            let snap = match cfg.read() {
                Ok(g) => g.clone(),
                Err(_) => return err_json(req, 500, "config lock poisoned"),
            };
            // No `host`: this machine, which is the common case of opening
            // the panel and just looking.
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
                Err(e) => err_json(req, 500, &format!("serialize: {e}")),
            }
        }

        "/api/files/transferencias" => match serde_json::to_string(&files::transfers()) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serialize: {e}")),
        },

        "/api/screen" => {
            let snap = match cfg.read() {
                Ok(g) => g.clone(),
                Err(_) => return err_json(req, 500, "config lock poisoned"),
            };
            match serde_json::to_string(&screen::plans(&snap)) {
                Ok(b) => ok_json(req, b),
                Err(e) => err_json(req, 500, &format!("serialize: {e}")),
            }
        }

        "/api/changelog" => match serde_json::to_string(&changelog::entries()) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serialize: {e}")),
        },

        "/api/layout" => {
            let snap = match cfg.read() {
                Ok(g) => g.clone(),
                Err(_) => return err_json(req, 500, "config lock poisoned"),
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
            let edges: Vec<_> = snap
                .border_edges_full()
                .into_iter()
                .map(|(e, t, ok)| serde_json::json!({ "edge": e, "target": t, "active": ok }))
                .collect();
            let body = serde_json::json!({
                "hosts": hosts,
                "edges": edges,
                "border_enabled": snap.input.border.enabled,
                "follow_enabled": snap.input.follow.enabled,
                "follow_leader": snap.input.follow.leader,
                "hidpp_paused_s": hidpp::pause_remaining_s(),
                "devices": snap.devices.iter().map(|d| d.name.clone()).collect::<Vec<_>>(),
            });
            ok_json(req, body.to_string())
        }

        "/api/processes" => {
            let q = req.url().split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
            let snap = match cfg.read() {
                Ok(g) => g.clone(),
                Err(_) => return err_json(req, 500, "config lock poisoned"),
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
                Err(e) => err_json(req, 500, &format!("serialize: {e}")),
            }
        }

        "/api/controls" => match serde_json::to_string(&hidpp::controls()) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serialize: {e}")),
        },

        "/api/devices" => match serde_json::to_string(&hidpp::survey()) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serialize: {e}")),
        },

        "/api/config" => match serde_json::to_string(&snap_cfg) {
            Ok(b) => ok_json(req, b),
            Err(e) => err_json(req, 500, &format!("serialize: {e}")),
        },

        "/setup" | "/setup.html" => respond(
            req,
            Response::from_string(SETUP).with_header(html_header()).with_header(nostore()),
        ),

        "/" | "/index.html" => {
            // Rendered per request: if refresh_seconds changes via PATCH,
            // the page reflects it without restarting the daemon.

            let page = PAGE.replace("__REFRESH__", &snap_cfg.refresh_seconds.to_string());
            respond(
                req,
                Response::from_string(page).with_header(html_header()).with_header(nostore()),
            )
        }

        _ => err_json(req, 404, "not found"),
    }
}
