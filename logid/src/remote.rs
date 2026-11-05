//! Executar uma linha de shell num host -- aqui ou no par.
//!
//! Extraido na terceira vez que apareceu. O `probe::detalhe` e o
//! `agent` ja montavam `ssh` com as mesmas quatro opcoes mais o contorno
//! de resolucao, e o gerenciador de processos precisaria do mesmo. A
//! quarta copia seria onde uma delas esqueceria o `ssh_addr_opts` e
//! falharia so no Windows -- que e exatamente como esse bug apareceu da
//! primeira vez.
//!
//! Quem chama escolhe o comando conforme o SO do ALVO (`HostCfg::os`),
//! nao conforme o nosso: rodar PowerShell contra o Ubuntu falha calado.

use crate::config::{HostCfg, HostOs};

/// Roda `cmd` no host e devolve o stdout.
///
/// Local: pelo shell desta maquina. Remoto: por SSH, com o endereco ja
/// resolvido por nos (ver `probe::ssh_addr_opts`).
pub fn run(h: &HostCfg, cmd: &str, timeout_s: u32) -> Result<String, String> {
    let out = if h.is_local() {
        let (prog, flags): (&str, &[&str]) = if cfg!(windows) {
            ("powershell", &["-NoProfile", "-Command"])
        } else {
            ("sh", &["-c"])
        };
        crate::sys::child(prog)
            .args(flags)
            .arg(cmd)
            .output()
            .map_err(|e| format!("executar local: {e}"))?
    } else {
        let alias = h.ssh.as_deref().ok_or_else(|| {
            format!("host '{}' nao tem `ssh` na config: nao ha como rodar nada la", h.name)
        })?;
        let mut args: Vec<String> = vec![
            "-o".into(), "BatchMode=yes".into(),
            "-o".into(), format!("ConnectTimeout={timeout_s}"),
            "-o".into(), "LogLevel=ERROR".into(),
        ];
        args.extend(crate::probe::ssh_addr_opts(&h.addr));
        args.push(alias.to_string());
        args.push(cmd.to_string());
        crate::sys::child("ssh").args(&args).output().map_err(|e| format!("ssh: {e}"))?
    };

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() && stdout.trim().is_empty() {
        // O OpenSSH do Windows imprime aviso de post-quantum em stderr; a
        // mensagem util e a ultima linha que nao seja isso.
        let err = String::from_utf8_lossy(&out.stderr)
            .lines()
            .filter(|l| {
                !l.contains("post-quantum") && !l.contains("openssh.com") && !l.contains("store now")
            })
            .filter(|l| !l.trim().is_empty())
            .last()
            .unwrap_or("comando falhou sem mensagem")
            .to_string();
        return Err(err);
    }
    Ok(stdout)
}

/// O shell do host entende PowerShell?
pub fn is_powershell(h: &HostCfg) -> bool {
    h.os() == HostOs::Windows
}

/// Linha de comando para invocar o NOSSO `logid` no par, por SSH.
///
/// Parece detalhe e não é: no Windows o `logid.exe` é
/// `windows_subsystem = "windows"` (sem console, para o daemon não
/// piscar janela), e **o PowerShell não espera executável GUI**. Sem
/// esperar, dois estragos:
///
/// 1. `$LASTEXITCODE` nunca é setado, então o SSH volta 0 e a origem
///    anuncia "recebeu" com o outro lado vazio.
/// 2. A sessão SSH fecha enquanto o processo ainda está lendo o stdin,
///    e o que sobra é uma corrida.
///
/// Medido: três envios de texto sem redirecionar a saída chegaram
/// **duas** vezes; com redirecionamento, três de três. Payload pequeno
/// costuma ganhar a corrida, e foi por isso que o clipboard de texto
/// parecia funcionar -- um `tar` é maior e perde mais.
///
/// `| Out-String | Write-Output` obriga o PowerShell a consumir a saída
/// do processo, e consumir implica esperar o fim. `; exit $LASTEXITCODE`
/// devolve o código de verdade para o SSH.
pub fn comando_logid(h: &HostCfg, sub: &str) -> String {
    if is_powershell(h) {
        format!(
            r#"& "$env:LOCALAPPDATA\logid\logid.exe" {sub} | Out-String | Write-Output; exit $LASTEXITCODE"#
        )
    } else {
        // `~` expande no shell remoto; caminho absoluto exigiria saber
        // o nome do usuário de cada máquina.
        format!("~/.local/bin/logid {sub}")
    }
}
