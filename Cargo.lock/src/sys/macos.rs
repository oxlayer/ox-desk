//! Implementacao macOS do contrato em `mod.rs`.
//!
//! O que e igual ao Linux vem de `unix.rs` (hora local, diretorio de
//! config, home, construtor de filho). Aqui fica o que difere.

use crate::config::Config;
use crate::logs::Raw;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub use super::unix::*;

/// O mDNSResponder do macOS resolve `.local` nativamente, entao nao
/// existe o buraco que o `getent` tapa no Linux.
pub fn resolve_fallback(_addr: &str, _port: u16) -> Option<SocketAddr> {
    None
}

/// Estado de um `units` da config, que aqui nao e unit nem servico.
///
/// Mesma postura do Windows: tenta o gerenciador do SO (launchd) e, se
/// nao houver nada com aquele rotulo, cai para "existe processo com esse
/// nome". Os dois casos aparecem -- `com.apple.screensharing` e servico
/// do launchd, `logid` e processo solto na sessao do usuario.
pub fn unit_state(unit: &str) -> String {
    // `launchctl list <label>` sai 0 quando o rotulo existe.
    let visto = super::unix::child("launchctl")
        .args(["list", unit])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if visto {
        return "active".into();
    }
    let rodando = super::unix::child("pgrep")
        .args(["-x", unit])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if rodando {
        "active".into()
    } else {
        "inactive".into()
    }
}

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const KEEP_LINES: usize = 500;

/// `~/Library/Logs/logid/logid.log` -- o lugar que o Console.app abre.
pub fn log_path() -> PathBuf {
    home_dir().join("Library").join("Logs").join("logid").join("logid.log")
}

/// Sem journal, o daemon timbra e persiste as proprias linhas.
///
/// Rotaciona por tamanho, igual ao Windows: um daemon residente que
/// escreve borda e bateria enche arquivo, e log que cresce sem limite
/// deixa de ser log e passa a ser problema de disco.
pub fn log_sink(line: &str) {
    let p = log_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&p).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            let kept: Vec<&str> = text.lines().rev().take(KEEP_LINES).collect();
            let body: String = kept.into_iter().rev().map(|l| format!("{l}\n")).collect();
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
        return Raw { unit: label, lines: vec![], error: Some("ainda nao ha arquivo de log".into()) };
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

/// Conseguimos falar com um receptor HID++?
///
/// Nesta maquina a resposta hoje e **nao**, e por um motivo de hardware:
/// o MacBook nao tem receptor Logitech USB -- mouse e teclado chegam nele
/// por Bluetooth, e o HID++ deste projeto fala com o receptor. O
/// `hidapi` ainda e consultado em vez de devolver `false` fixo, porque
/// plugar um receptor resolveria, e nesse dia o codigo nao deve precisar
/// de mudanca.
pub fn device_access() -> bool {
    hidpp::hidapi::HidApi::new()
        .ok()
        .map(|api| hidpp::Receiver::open(&api).is_ok())
        .unwrap_or(false)
}

/// Nao ha vigilancia de borda no macOS -- e nao e so falta de codigo.
///
/// Ler o cursor aqui e facil (`CGEventGetLocation`) e o tempo de ocio
/// tambem (`CGEventSourceSecondsSinceLastEventType`). O que falta e o
/// outro lado: sem receptor, nao ha como **empurrar** o dispositivo para
/// outro canal por HID++. Uma borda que detecta e nao troca seria pior
/// que nenhuma -- prometeria uma coisa que nao acontece, que e o padrao
/// de bug que este projeto ja pagou duas vezes (a flag `backfill_gate`
/// que ninguem lia, o `langfuse` que engolia erro).
///
/// Para SAIR do Mac, use o botao Easy-Switch. Para o Mac RECEBER, basta
/// outra maquina empurrar -- e para isso o `focus::plan` de la exige um
/// `agent` aqui, que so faz sentido quando houver receptor.
pub fn spawn_border(cfg: Arc<RwLock<Config>>) {
    let ligada = cfg.read().map(|g| g.input.border.enabled).unwrap_or(false);
    if ligada {
        gripe!(
            "border: ligada na config, mas nao ha vigilancia de borda no macOS -- \
             sem receptor Logitech nao ha como empurrar o dispositivo por HID++. \
             Saia pelo botao Easy-Switch. Ver sys/macos.rs."
        );
    }
}

/// Clientes de tela conhecidos, em ordem de preferencia.
///
/// VNC e nativo: o `open vnc://` abre o Compartilhamento de Tela da
/// Apple, que ja vem instalado. RDP nao vem nada -- o caminho e o
/// `Windows App` (ex-Microsoft Remote Desktop), por
/// `brew install --cask windows-app`.
pub fn screen_clients(kind: &str) -> Vec<(&'static str, Vec<&'static str>)> {
    match kind {
        "vnc" => vec![("open", vec!["vnc://{host}:{port}"])],
        // O esquema `rdp://` com `full address` e o que o cliente da
        // Microsoft registra. NAO verificado ainda -- o app nao esta
        // instalado; quando estiver, o blocker da UI muda de "sem
        // cliente" para o que acontecer de fato.
        "rdp" => vec![("open", vec!["rdp://full%20address=s:{host}:{port}"])],
        _ => vec![],
    }
}

pub fn screen_client_dirs() -> Vec<PathBuf> {
    vec![PathBuf::from("/usr/bin"), PathBuf::from("/Applications")]
}
