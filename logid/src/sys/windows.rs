//! Implementacao Windows do contrato em `mod.rs`.

use crate::config::Config;
use crate::logs::Raw;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// O resolver do Windows ja fala mDNS desde o 10, entao nao existe o
/// buraco que o `getent` tapa no Linux: `to_socket_addrs` basta.
pub fn resolve_fallback(_addr: &str, _port: u16) -> Option<SocketAddr> {
    None
}

/// Estado de um `units` da config, que no Windows nao e unit nenhum.
///
/// O nome do campo fica (a config e a mesma nas duas maquinas), mas a
/// leitura e outra: primeiro tentamos um servico de verdade, e so se ele
/// nao existir caimos para "existe processo com esse nome". Os dois casos
/// aparecem na pratica -- `sshd` e servico, `logid` e processo solto na
/// sessao do usuario.
pub fn unit_state(unit: &str) -> String {
    match service_state(unit) {
        Some(s) => s,
        None => {
            if process_running(unit) {
                "active".into()
            } else {
                "inactive".into()
            }
        }
    }
}

/// `None` = nao existe servico com esse nome (ERROR_SERVICE_DOES_NOT_EXIST).
fn service_state(name: &str) -> Option<String> {
    let out = child("sc").args(["query", name]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return None;
    }
    // O rotulo da linha e traduzido ("ESTADO" num Windows pt-BR), mas o
    // nome do estado vem do SCM como constante em ingles. Mesmo assim
    // casamos tambem pelo codigo numerico, que nao depende de locale.
    let up = text.to_ascii_uppercase();
    let by_name = [
        ("RUNNING", "active"),
        ("STOPPED", "inactive"),
        ("START_PENDING", "activating"),
        ("STOP_PENDING", "deactivating"),
        ("PAUSED", "inactive"),
    ]
    .iter()
    .find(|(tok, _)| up.contains(tok))
    .map(|(_, s)| s.to_string());
    if by_name.is_some() {
        return by_name;
    }
    for (code, state) in [(": 4", "active"), (": 1", "inactive"), (": 2", "activating")] {
        if up.contains(code) {
            return Some(state.into());
        }
    }
    Some("unknown".into())
}

fn process_running(name: &str) -> bool {
    let exe = if name.to_ascii_lowercase().ends_with(".exe") {
        name.to_string()
    } else {
        format!("{name}.exe")
    };
    let out = child("tasklist")
        .args(["/FO", "CSV", "/NH", "/FI", &format!("IMAGENAME eq {exe}")])
        .output();
    let Ok(out) = out else { return false };
    // Quando nao casa, o tasklist escreve "INFO: nenhuma tarefa..." --
    // mensagem traduzida. Procurar o nome do exe evita depender disso.
    String::from_utf8_lossy(&out.stdout)
        .to_ascii_lowercase()
        .contains(&exe.to_ascii_lowercase())
}

#[repr(C)]
#[derive(Default)]
struct SysTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    ms: u16,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLocalTime(st: *mut SysTime);
}

/// Hora local direto do Win32 -- sem `date +%z` nem aritmetica de fuso,
/// porque aqui o SO ja entrega o relogio de parede local.
pub fn hhmmss() -> String {
    let mut st = SysTime::default();
    unsafe { GetLocalTime(&mut st) };
    format!("{:02}:{:02}:{:02}", st.hour, st.minute, st.second)
}

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const KEEP_LINES: usize = 500;

pub fn log_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("logid").join("logid.log")
}

/// Sem journal, o daemon timbra e persiste as proprias linhas.
///
/// Rotaciona por tamanho: um daemon residente que escreve borda e
/// bateria enche arquivo, e um log que cresce sem limite deixa de ser log
/// e passa a ser problema de disco.
pub fn log_sink(line: &str) {
    let p = log_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&p).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            let kept: Vec<&str> = text.lines().rev().take(KEEP_LINES).collect();
            let body: String =
                kept.into_iter().rev().map(|l| format!("{l}\n")).collect();
            let _ = std::fs::write(&p, body);
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{} {}", hhmmss(), line);
    }
}

pub fn log_lines(n: usize) -> Raw {
    let p = log_path();
    let label = p.to_string_lossy().to_string();
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Raw {
            unit: label,
            lines: vec![],
            error: Some("ainda nao ha arquivo de log".into()),
        };
    };
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(n);
    let lines = all[start..]
        .iter()
        .filter_map(|l| l.split_once(' '))
        .map(|(t, m)| (t.to_string(), m.to_string()))
        .collect();
    Raw { unit: label, lines, error: None }
}

pub fn spawn_border(cfg: Arc<RwLock<Config>>) {
    crate::border_win::spawn(cfg);
}

/// Conseguimos falar com o receptor?
///
/// No Windows nao existe `/dev/hidraw` nem ACL de `uaccess`: o acesso a
/// colecao do fabricante (usage page 0xFF00) e liberado por padrao, e o
/// que resta e abrir e ver. Checar a coisa certa importa -- a versao
/// Linux deste check procura arquivo em `/dev`, e aqui ela responderia
/// "sem acesso" para sempre, bloqueando todo plano de troca.
pub fn device_access() -> bool {
    hidpp::hidapi::HidApi::new()
        .ok()
        .map(|api| hidpp::Receiver::open(&api).is_ok())
        .unwrap_or(false)
}

/// Onde mora a config: `%APPDATA%\logi`.
///
/// `HOME` nao existe no Windows. A versao Linux cai para `/tmp` quando
/// nao a encontra -- aqui isso significaria gravar a config num caminho
/// sem sentido e perde-la no proximo boot.
pub fn config_dir() -> PathBuf {
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("logi")
}

/// Ponto de partida do navegador de arquivos.
pub fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| "C:\\".into())
}

/// Clientes de tela conhecidos, em ordem de preferencia.
///
/// RDP ja vem no Windows (`mstsc`), mas ele nao aceita som nem microfone
/// por linha de comando -- isso exige gerar um arquivo `.rdp`, o que nao
/// vale a complexidade por enquanto. VNC nao vem nenhum: e o que falta
/// para alcancar o Mac daqui.
pub fn screen_clients(kind: &str) -> Vec<super::ClientSpec> {
    use super::ClientSpec;
    match kind {
        "rdp" => vec![ClientSpec {
            prog: "mstsc.exe",
            args: &["/v:{host}:{port}"],
            audio: None,
            mic: None,
            view_only: None,
        }],
        "vnc" => vec![
            ClientSpec {
                prog: "vncviewer.exe",
                args: &["{host}:{port}"],
                audio: None,
                mic: None,
                view_only: Some(&["-ViewOnly"]),
            },
            ClientSpec {
                prog: "tvnviewer.exe",
                args: &["{host}::{port}"],
                audio: None,
                mic: None,
                view_only: Some(&["-viewonly"]),
            },
        ],
        _ => vec![],
    }
}

/// Instaladores nao poem viewer de VNC no PATH; estes sao os lugares
/// onde o RealVNC e o TightVNC caem por padrao.
pub fn screen_client_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(base) = std::env::var(key) {
            v.push(PathBuf::from(&base).join("RealVNC").join("VNC Viewer"));
            v.push(PathBuf::from(&base).join("TightVNC"));
            v.push(PathBuf::from(&base).join("uvnc bvba").join("UltraVNC"));
        }
    }
    v
}

/// Cria processo sem alocar console.
///
/// O daemon e subsistema `windows` (sem console proprio, para nao abrir
/// janela preta a cada login). A consequencia e que **cada** filho de
/// console que ele cria ganha um console NOVO -- e o refresher de pares
/// chama `ssh` a cada 6 s, o que virava uma janela de cmd piscando sem
/// parar na tela do usuario. Reclamado, e com razao: parece coisa
/// quebrada.
///
/// `CREATE_NO_WINDOW` resolve na origem. Nao atrapalha o que de fato tem
/// interface: em app grafico (o `mstsc`, por exemplo) a flag so impede o
/// console, nao as janelas dele.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn child(prog: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new(prog);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}
