//! Implementacao Windows do contrato em `mod.rs`.

use crate::config::Config;
use crate::logs::Raw;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
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
    let out = Command::new("sc").args(["query", name]).output().ok()?;
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
    let out = Command::new("tasklist")
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
