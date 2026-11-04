//! Tela remota: nao servimos tela, despachamos o cliente certo.
//!
//! Spec: ../specs/006-screen-share.md
//!
//! ## A decisao (03/11, do usuario): nativo de cada SO
//!
//! Cada maquina ja sabe ser controlada, e por um protocolo diferente:
//! o GNOME fala RDP pelo `gnome-remote-desktop` (ja ativo, 3389 exposta
//! na LAN, TLS e credenciais configurados), o Windows fala RDP nativo, o
//! macOS fala VNC pelo Compartilhamento de Tela. Nao existe protocolo
//! que os tres sirvam de fabrica.
//!
//! A alternativa considerada era um RustDesk auto-hospedado: um cliente
//! igual nos tres, e so conexao de saida -- o que importaria num Mac com
//! MDM. Perdeu porque e uma dependencia externa grande e um relay a
//! manter, enquanto o nativo nao sobe daemon novo em lugar nenhum.
//!
//! ## O que este modulo faz, e o que nao faz
//!
//! **Nao** implementa protocolo de tela. Ele responde duas perguntas --
//! "a tela daquele host esta alcancavel?" e "existe cliente aqui para
//! falar com ela?" -- e, a pedido, lanca o cliente. A parte difícil
//! (video, input, TLS) e dos programas que ja existem.
//!
//! Por isso o `GET` e separado do `POST`: a UI precisa saber se o botao
//! vai funcionar ANTES de a pessoa clicar. Um botao que abre nada e pior
//! que um botao desabilitado com motivo escrito do lado.

use crate::config::Config;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Stdio;

#[derive(Debug, Serialize)]
pub struct Plan {
    pub host: String,
    /// rdp | vnc
    pub kind: String,
    pub port: u16,
    /// A porta respondeu agora.
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<f64>,
    /// Cliente escolhido, se achamos algum.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// Linha de comando que seria executada -- para a UI poder mostrar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Pronto para clicar.
    pub ready: bool,
    /// Por que nao esta pronto.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
}

/// Procura um executavel no PATH e nos diretorios que a plataforma
/// indicar. No Windows tenta tambem sem `.exe`, porque o PATHEXT
/// resolve isso para o shell mas nao para nos.
fn find_program(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.extend(crate::sys::screen_client_dirs());
    for d in dirs {
        let c = d.join(name);
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

/// Substitui os marcadores do template do cliente.
///
/// `{u_flag}` existe porque o freerdp quer `/u:nome` como UM argumento, e
/// quando nao ha usuario o argumento tem de **desaparecer** -- passar
/// `/u:` vazio faz ele pedir usuario em prompt e travar sem terminal.
fn render(args: &[&str], host: &str, port: u16, user: Option<&str>) -> Vec<String> {
    let user_at = user.map(|u| format!("{u}@")).unwrap_or_default();
    args.iter()
        .filter_map(|a| {
            if *a == "{u_flag}" {
                return user.map(|u| format!("/u:{u}"));
            }
            Some(
                a.replace("{host}", host)
                    .replace("{port}", &port.to_string())
                    .replace("{user_at}", &user_at),
            )
        })
        .collect()
}

fn plan_for(cfg: &Config, name: &str) -> Option<Plan> {
    let h = cfg.hosts.iter().find(|x| x.name == name)?;
    let sc = h.screen.as_ref()?;
    // A propria maquina nao se controla por tela remota -- e a tela que
    // voce esta olhando.
    if h.is_local() {
        return None;
    }
    let (reachable, ms) = crate::probe::tcp_probe(&h.addr, sc.port);
    let user = sc.user.as_deref();

    let mut client = None;
    let mut command = None;
    for (prog, args) in crate::sys::screen_clients(&sc.kind) {
        if let Some(found) = find_program(prog) {
            let mut cmd = vec![found.to_string_lossy().into_owned()];
            cmd.extend(render(&args, &h.addr, sc.port, user));
            client = Some(prog.to_string());
            command = Some(cmd);
            break;
        }
    }

    let blocker = if client.is_none() {
        let tentados: Vec<&str> =
            crate::sys::screen_clients(&sc.kind).into_iter().map(|(p, _)| p).collect();
        Some(if tentados.is_empty() {
            format!("protocolo '{}' nao tem cliente conhecido aqui", sc.kind)
        } else {
            format!("sem cliente {} instalado aqui (procurei: {})", sc.kind, tentados.join(", "))
        })
    } else if !reachable {
        Some(format!(
            "{}:{} nao respondeu -- a tela do host esta desligada ou bloqueada",
            h.addr, sc.port
        ))
    } else {
        None
    };

    Some(Plan {
        host: h.name.clone(),
        kind: sc.kind.clone(),
        port: sc.port,
        reachable,
        ms: ms.map(|v| (v * 10.0).round() / 10.0),
        ready: blocker.is_none(),
        client,
        command,
        blocker,
    })
}

/// Um plano por host que tenha `[hosts.screen]` e nao seja este.
pub fn plans(cfg: &Config) -> Vec<Plan> {
    // Em paralelo: cada plano faz um probe TCP com timeout de 1,5 s, e em
    // serie tres hosts offline custariam 4,5 s de espera na abertura do
    // painel.
    std::thread::scope(|s| {
        let hs: Vec<_> = cfg
            .hosts
            .iter()
            .map(|h| {
                let name = h.name.clone();
                s.spawn(move || plan_for(cfg, &name))
            })
            .collect();
        hs.into_iter().filter_map(|j| j.join().ok().flatten()).collect()
    })
}

/// Lanca o cliente. Devolve o que foi executado, para o log e para a UI.
pub fn launch(cfg: &Config, host: &str) -> Result<String, String> {
    let plan = plan_for(cfg, host)
        .ok_or_else(|| format!("host '{host}' nao tem `[hosts.screen]` na config"))?;
    if let Some(b) = &plan.blocker {
        return Err(b.clone());
    }
    let cmd = plan.command.ok_or("sem comando")?;
    let (prog, args) = cmd.split_first().ok_or("comando vazio")?;

    // `spawn` e nao `output`: o cliente de tela e interativo e vive
    // minutos ou horas. Esperar por ele travaria a thread do servidor.
    // stdio ignorado para o filho nao herdar o nosso e manter pipe aberto.
    crate::sys::child(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("nao consegui executar {prog}: {e}"))?;

    let linha = cmd.join(" ");
    say!("screen: abrindo {host} ({}) -- {linha}", plan.kind);
    Ok(linha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_de_usuario_some_quando_nao_ha_usuario() {
        // `/u:` vazio faz o freerdp pedir usuario em prompt e travar sem
        // terminal -- o argumento tem de desaparecer, nao ficar vazio.
        let args = vec!["/v:{host}:{port}", "{u_flag}", "+clipboard"];
        assert_eq!(
            render(&args, "pc.local", 3389, None),
            vec!["/v:pc.local:3389", "+clipboard"]
        );
        assert_eq!(
            render(&args, "pc.local", 3389, Some("user")),
            vec!["/v:pc.local:3389", "/u:user", "+clipboard"]
        );
    }

    #[test]
    fn uri_recebe_usuario_colado() {
        let args = vec!["rdp://{user_at}{host}:{port}"];
        assert_eq!(render(&args, "pc.local", 3389, Some("v")), vec!["rdp://v@pc.local:3389"]);
        assert_eq!(render(&args, "pc.local", 3389, None), vec!["rdp://pc.local:3389"]);
    }
}
