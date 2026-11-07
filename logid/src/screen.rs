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

/// O que a pessoa pediu na UI, no estilo do Parsec.
#[derive(Debug, Clone, Copy)]
pub struct Opts {
    /// Entrar com teclado e mouse. `false` = so olhar.
    pub control: bool,
    /// Trazer o som da outra maquina.
    pub audio: bool,
    /// Mandar o microfone daqui para la.
    pub mic: bool,
}

impl Default for Opts {
    fn default() -> Self {
        // Controlar e o caso comum; som e microfone nao, porque ligar
        // audio sem pedir e o tipo de surpresa que assusta numa reuniao.
        Opts { control: true, audio: false, mic: false }
    }
}

/// O que o cliente escolhido consegue honrar. A UI usa isto para
/// desabilitar o que nao adianta marcar, com o motivo do lado.
#[derive(Debug, Serialize, Default)]
pub struct Caps {
    pub audio: bool,
    pub mic: bool,
    /// Consegue entrar SEM teclado e mouse.
    pub view_only: bool,
}

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
    /// O que este cliente sabe fazer.
    pub caps: Caps,
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
/// Escreve o `.rdp` num temporario e devolve o caminho.
///
/// Arquivo e nao argumento porque ha cliente que so aceita assim -- e
/// porque e por ele que som e microfone passam. Nome fixo por host: um
/// arquivo novo a cada clique encheria o temp sem necessidade.
fn write_rdp(modelo: &str, host: &str, port: u16, user: Option<&str>, opts: Opts) -> Option<String> {
    let user_line = user.map(|u| format!("username:s:{u}\n")).unwrap_or_default();
    let corpo = modelo
        .replace("{host}", host)
        .replace("{port}", &port.to_string())
        .replace("{user_line}", &user_line)
        // 0 = tocar aqui, 2 = nao tocar em lugar nenhum.
        .replace("{audiomode}", if opts.audio { "0" } else { "2" })
        .replace("{audiocapture}", if opts.mic { "1" } else { "0" });
    let seguro: String =
        host.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let p = std::env::temp_dir().join(format!("logi-{seguro}.rdp"));
    std::fs::write(&p, corpo).ok()?;
    Some(p.to_string_lossy().into_owned())
}

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

fn plan_for(cfg: &Config, name: &str, opts: Opts) -> Option<Plan> {
    let h = cfg.hosts.iter().find(|x| x.name == name)?;
    let sc = h.screen.as_ref()?;
    // A propria maquina nao se controla por tela remota -- e a tela que
    // voce esta olhando.
    if h.is_local() {
        return None;
    }
    // O nosso protocolo e QUIC, ou seja **UDP**: um probe TCP diria
    // "fechado" para sempre e o botao nunca acenderia. E o host dele
    // sobe sob demanda, entao nem estar no ar e pre-requisito -- o que
    // importa e haver canal SSH para levanta-lo.
    let (reachable, ms) = if sc.kind == "logistream" {
        (h.ssh.is_some(), None)
    } else {
        crate::probe::tcp_probe(&h.addr, sc.port)
    };
    let user = sc.user.as_deref();

    let mut client = None;
    let mut command = None;
    let mut caps = Caps::default();
    for spec in crate::sys::screen_clients(&sc.kind) {
        let Some(found) = find_program(spec.prog) else { continue };
        let mut args: Vec<&str> = spec.args.to_vec();
        if opts.audio {
            if let Some(extra) = spec.audio {
                args.extend_from_slice(extra);
            }
        }
        if opts.mic {
            if let Some(extra) = spec.mic {
                args.extend_from_slice(extra);
            }
        }
        if !opts.control {
            if let Some(extra) = spec.view_only {
                args.extend_from_slice(extra);
            }
        }
        let arquivo = spec.file.and_then(|m| write_rdp(m, &h.addr, sc.port, user, opts));
        let mut cmd = vec![found.to_string_lossy().into_owned()];
        cmd.extend(
            render(&args, &h.addr, sc.port, user)
                .into_iter()
                .map(|a| match &arquivo {
                    Some(f) => a.replace("{file}", f),
                    None => a,
                })
                .collect::<Vec<_>>(),
        );
        caps = Caps {
            audio: spec.audio.is_some(),
            mic: spec.mic.is_some(),
            view_only: spec.view_only.is_some(),
        };
        client = Some(spec.prog.to_string());
        command = Some(cmd);
        break;
    }

    let blocker = if client.is_none() {
        let tentados: Vec<&str> =
            crate::sys::screen_clients(&sc.kind).into_iter().map(|c| c.prog).collect();
        Some(if tentados.is_empty() {
            format!("protocolo '{}' nao tem cliente conhecido aqui", sc.kind)
        } else {
            format!("sem cliente {} instalado aqui (procurei: {})", sc.kind, tentados.join(", "))
        })
    } else if !reachable {
        Some(if sc.kind == "logistream" {
            format!(
                "'{}' nao tem `ssh` na config: sem ele nao ha como levantar o host de tela la",
                h.name
            )
        } else {
            format!(
                "{}:{} nao respondeu -- a tela do host esta desligada ou bloqueada",
                h.addr, sc.port
            )
        })
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
        caps,
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
                s.spawn(move || plan_for(cfg, &name, Opts::default()))
            })
            .collect();
        hs.into_iter().filter_map(|j| j.join().ok().flatten()).collect()
    })
}

/// Lanca o cliente. Devolve o que foi executado, para o log e para a UI.
pub fn launch(cfg: &Config, host: &str, opts: Opts) -> Result<String, String> {
    let plan = plan_for(cfg, host, opts)
        .ok_or_else(|| format!("host '{host}' nao tem `[hosts.screen]` na config"))?;
    if let Some(b) = &plan.blocker {
        return Err(b.clone());
    }
    // O host de tela sobe sob demanda, antes de o cliente tentar
    // conectar. Capturar e codificar sem ninguem assistindo so gastaria
    // GPU na maquina do outro.
    if plan.kind == "logistream" {
        if let Some(h) = cfg.hosts.iter().find(|x| x.name == host) {
            if let Err(e) = acordar_host(h, plan.port) {
                return Err(format!("nao consegui levantar o host de tela: {e}"));
            }
        }
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
    let modo = format!(
        "{}{}{}",
        if opts.control { "controle" } else { "so olhar" },
        if opts.audio { " +som" } else { "" },
        if opts.mic { " +microfone" } else { "" }
    );
    say!("screen: abrindo {host} ({}, {modo}) -- {linha}", plan.kind);
    Ok(linha)
}

/// O que a aba precisa para falar com a ponte.
#[derive(serde::Serialize)]
pub struct Sessao {
    pub porta: u16,
    pub token: String,
}

/// Abre uma sessao em aba: acorda o host e sobe a `logistream-ponte`.
///
/// Sem a ponte instalada devolve erro, e a pagina cai no visualizador
/// nativo. Ver specs/016-sessao-e-visual.md.
pub fn abrir_sessao(cfg: &Config, host: &str) -> Result<Sessao, String> {
    let plan = plan_for(cfg, host, Opts::default())
        .ok_or_else(|| format!("host '{host}' nao tem `[hosts.screen]` na config"))?;
    if let Some(b) = &plan.blocker {
        return Err(b.clone());
    }
    if plan.kind != "logistream" {
        return Err(format!("'{host}' nao usa logistream"));
    }
    let h = cfg
        .hosts
        .iter()
        .find(|x| x.name == host)
        .ok_or_else(|| format!("host '{host}' nao esta na config"))?;
    let nome = if cfg!(windows) { "logistream-ponte.exe" } else { "logistream-ponte" };
    let ponte = find_program(nome).ok_or("logistream-ponte nao instalada")?;
    acordar_host(h, plan.port).map_err(|e| format!("nao consegui levantar o host de tela: {e}"))?;

    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("token: {e}"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let mut filho = crate::sys::child(&ponte.to_string_lossy())
        .arg(format!("{}:{}", h.addr, plan.port))
        .env("LOGI_PONTE_TOKEN", &token)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("nao consegui executar a ponte: {e}"))?;
    let saida = filho.stdout.take().ok_or("a ponte nao tem saida")?;
    let mut linha = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(saida), &mut linha)
        .map_err(|e| format!("ler a ponte: {e}"))?;
    let porta = linha
        .trim()
        .strip_prefix("porta ")
        .and_then(|p| p.parse().ok())
        .ok_or("a ponte nao disse a porta")?;
    // A ponte sai sozinha quando a aba fecha; esta thread so colhe o filho.
    std::thread::spawn(move || {
        let _ = filho.wait();
    });
    say!("screen: aba de {host} pela ponte na porta {porta}");
    Ok(Sessao { porta, token })
}

/// Garante que o `logistream-host` esta rodando na outra maquina.
///
/// Idempotente de proposito: o botao pode ser clicado duas vezes, e uma
/// segunda instancia competiria pela mesma porta e pela mesma captura.
///
/// Como acordar depende do SO do alvo, e a diferenca nao e cosmetica:
///
/// - **Unix**: `setsid nohup` solta o processo da sessao SSH. Sem isso
///   ele morre junto com o comando que o criou -- a mesma armadilha do
///   Windows matando a arvore ao desconectar (specs/001). O `setsid` e
///   condicional porque **o macOS nao o tem**: la a linha inteira
///   morria em "command not found" antes de chegar ao nohup.
/// - **Windows**: tarefa agendada com `/it`, e **nao** lancamento direto.
///   O SSH cai na sessao 0, e la o Desktop Duplication nao existe e o
///   `SendInput` nao chega a desktop nenhum: o processo subiria, nao
///   falharia, e entregaria uma tela que ninguem esta vendo. E a mesma
///   fronteira de sessao que obriga o `logid` a subir por `schtasks`.
pub(crate) fn acordar_host(h: &crate::config::HostCfg, porta: u16) -> Result<(), String> {
    let cmd = if crate::remote::is_powershell(h) {
        // Sem aspas duplas em lugar nenhum: tres bugs de quoting nesta
        // ponte ja custaram tempo (specs/002). Palavra nua vira string
        // no PowerShell, e `-ErrorAction SilentlyContinue` evita que o
        // "processo nao existe" vire erro.
        concat!(
            "if (Get-Process -Name logistream-host -ErrorAction SilentlyContinue) ",
            "{ Write-Output vivo } else { schtasks /run /tn logistream-host | Out-Null; ",
            "Start-Sleep -Seconds 2; ",
            "if (Get-Process -Name logistream-host -ErrorAction SilentlyContinue) ",
            "{ Write-Output vivo } else { Write-Output morto } }"
        )
        .to_string()
    } else if h.os.as_deref() == Some("macos") {
        // macOS: LaunchAgent, nao `nohup`.
        //
        // A captura de tela depende de permissao TCC (Gravacao de
        // Tela), concedida a quem LANCOU o processo. Vindo do SSH nao
        // ha concessao nenhuma -- e o AVFoundation, em vez de falhar,
        // **trava**. Medido: o proprio `ffmpeg -f avfoundation -i 1`
        // pendura para sempre por SSH.
        //
        // Mesma fronteira que obriga o Windows a usar tarefa agendada
        // com /IT em vez de servico.
        concat!(
            r#"pgrep -x logistream-host >/dev/null 2>&1 || "#,
            r#"launchctl kickstart "gui/$(id -u)/ai.logi.logistream-host" >/dev/null 2>&1; "#,
            r#"sleep 1; pgrep -x logistream-host >/dev/null && echo vivo || echo morto"#
        )
        .to_string()
    } else {
        format!(
            concat!(
                r#"pgrep -x logistream-host >/dev/null 2>&1 || "#,
                // `setsid` quando existe, `nohup` sozinho quando nao.
                //
                // O macOS NAO tem setsid, e a linha inteira morria com
                // "command not found" -- sem nunca chegar ao nohup. Em
                // Linux o setsid continua valendo: ele tira o processo
                // da sessao do SSH, e sem isso ele cai junto quando a
                // conexao fecha.
                r#"(S=; command -v setsid >/dev/null 2>&1 && S=setsid; "#,
                r#"$S nohup "$HOME/.local/bin/logistream-host" {} "#,
                r#">"$HOME/.local/state/logistream-host.log" 2>&1 &); "#,
                r#"sleep 1; pgrep -x logistream-host >/dev/null && echo vivo || echo morto"#
            ),
            porta
        )
    };
    let saida = crate::remote::run(h, &cmd, 12)?;
    if saida.contains("vivo") {
        say!("screen: host de tela de pe em {}", h.name);
        return Ok(());
    }
    // A dica de onde olhar muda com o SO, e dar a errada manda a pessoa
    // procurar arquivo que nao existe.
    Err(if crate::remote::is_powershell(h) {
        format!(
            "o host de tela nao subiu em {}: confira `schtasks /query /tn logistream-host` \
             la -- a tarefa precisa existir e a sessao estar aberta (ela roda com /it)",
            h.name
        )
    } else {
        format!(
            "o processo nao subiu em {} (veja ~/.local/state/logistream-host.log la)",
            h.name
        )
    })
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
