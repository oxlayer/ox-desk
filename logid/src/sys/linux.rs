//! Implementacao Linux do contrato em `mod.rs`.

use crate::config::Config;
use crate::logs::Raw;
pub use super::unix::*;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::Command;
use std::sync::{Arc, RwLock};

const UNIT: &str = "logid.service";

/// Resolve por `getent` quando o resolver da libc desiste.
///
/// `to_socket_addrs` usa getaddrinfo com AF_UNSPEC, e isso **falha** para
/// `.local` nesta maquina: `mdns4_minimal` com `[NOTFOUND=return]` no
/// nsswitch aborta a cadeia na consulta AAAA. Comprovado: `ssh-keyscan
/// windows-host.local` da "Temporary failure in name resolution" enquanto
/// `getent hosts` funciona.
pub fn resolve_fallback(addr: &str, port: u16) -> Option<SocketAddr> {
    let out = Command::new("getent").arg("hosts").arg(addr).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let ip = text.split_whitespace().next()?;
    (ip, port).to_socket_addrs().ok()?.next()
}

pub fn unit_state(unit: &str) -> String {
    Command::new("systemctl")
        .args(["--user", "is-active", unit])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// Aqui o journal timbra e persiste; o daemon so escreve em stdout.
pub fn log_sink(_line: &str) {}

pub fn log_lines(n: usize) -> Raw {
    let out = Command::new("journalctl")
        .args(["--user", "-u", UNIT, "-n", &n.to_string(), "--no-pager", "-o", "short-iso"])
        .output();

    let Ok(out) = out else {
        return Raw {
            unit: UNIT.into(),
            lines: vec![],
            error: Some("nao consegui executar journalctl".into()),
        };
    };

    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.starts_with("--") {
            continue; // cabecalho do journal
        }
        // "2026-11-03T18:51:18-0300 host logid[123]: mensagem"
        let Some((time, rest)) = line.split_once(' ') else { continue };
        let msg = match rest.split_once("]: ") {
            Some((_, m)) => m,
            None => rest.split_once(": ").map(|(_, m)| m).unwrap_or(rest),
        };
        lines.push((time.get(11..19).unwrap_or(&time).to_string(), msg.to_string()));
    }
    Raw { unit: UNIT.into(), lines, error: None }
}

pub fn spawn_border(cfg: Arc<RwLock<Config>>) {
    crate::border::spawn(cfg);
}

/// Conseguimos abrir algum `/dev/hidraw`?
///
/// A regra do Solaar concede por `TAG+="uaccess"`, que vira ACL para o
/// usuario logado. Pegadinha paga: se os nos nasceram ANTES de a regra
/// existir, nenhuma ACL foi aplicada e so `udevadm trigger` resolve.
pub fn device_access() -> bool {
    let Ok(entries) = std::fs::read_dir("/dev") else { return false };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(n) = name.to_str() else { continue };
        if n.starts_with("hidraw") && std::fs::File::open(e.path()).is_ok() {
            return true;
        }
    }
    false
}

/// Clientes de tela conhecidos, em ordem de preferencia.
///
/// O `xfreerdp3` vem ANTES do `gnome-connections` porque e o unico que
/// expoe som e microfone por linha de comando -- e sao justamente as
/// opcoes que a UI oferece. O gnome-connections continua na lista como
/// queda: ele fala RDP e VNC e ja vem instalado, so nao deixa escolher
/// nada.
pub fn screen_clients(kind: &str) -> Vec<super::ClientSpec> {
    use super::ClientSpec;
    match kind {
        // `/dynamic-resolution` faz o host remoto acompanhar a janela; sem
        // isso a sessao nasce com a resolucao do servidor e fica com
        // barras. `+clipboard` e o que faz copiar e colar entre as duas
        // maquinas funcionar dentro da sessao.
        "rdp" => vec![
            ClientSpec {
                prog: "xfreerdp3",
                args: &["/v:{host}:{port}", "{u_flag}", "+clipboard", "/dynamic-resolution"],
                audio: Some(&["/sound"]),
                mic: Some(&["/microphone"]),
                view_only: None,
                file: None,
            },
            ClientSpec {
                prog: "xfreerdp",
                args: &["/v:{host}:{port}", "{u_flag}", "+clipboard", "/dynamic-resolution"],
                audio: Some(&["/sound"]),
                mic: Some(&["/microphone"]),
                view_only: None,
                file: None,
            },
            ClientSpec {
                prog: "gnome-connections",
                args: &["rdp://{user_at}{host}:{port}"],
                audio: None,
                mic: None,
                view_only: None,
                file: None,
            },
            ClientSpec {
                prog: "remmina",
                args: &["-c", "rdp://{user_at}{host}:{port}"],
                audio: None,
                mic: None,
                view_only: None,
                file: None,
            },
        ],
        "vnc" => vec![
            // O vncviewer vem primeiro por causa do `-ViewOnly`: e o unico
            // jeito, deste lado, de entrar sem teclado e mouse.
            ClientSpec {
                prog: "vncviewer",
                args: &["{host}:{port}"],
                audio: None,
                mic: None,
                view_only: Some(&["-ViewOnly"]),
                file: None,
            },
            ClientSpec {
                prog: "gnome-connections",
                args: &["vnc://{host}:{port}"],
                audio: None,
                mic: None,
                view_only: None,
                file: None,
            },
            ClientSpec {
                prog: "remmina",
                args: &["-c", "vnc://{host}:{port}"],
                audio: None,
                mic: None,
                view_only: None,
                file: None,
            },
        ],
        _ => vec![],
    }
}

/// Onde procurar clientes fora do PATH. No Linux o PATH basta.
pub fn screen_client_dirs() -> Vec<std::path::PathBuf> {
    vec![]
}
