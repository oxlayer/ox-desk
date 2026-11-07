//! Rodar programas e falar com a rede, com teto de tempo.
//!
//! Spec: ../../specs/015-ota.md
//!
//! Gemeo do `rodar` de `logid/src/setup_comum.rs`, e de proposito: este
//! binario nao linka o daemon (nao ha `lib.rs` lá) e transformar o
//! `logid` em biblioteca para compartilhar 40 linhas de espera por
//! processo custaria mais do que paga. Se um terceiro cliente aparecer,
//! o lugar certo e uma crate `comum/`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
fn comando(prog: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(prog);
    // Sem janela: isto roda por tarefa agendada, e um console piscando
    // no meio da sessao de alguem e exatamente o que o host de tela
    // deixou de fazer (ver o comentario do `.cmd` em deploy.sh).
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

#[cfg(not(windows))]
fn comando(prog: &str) -> Command {
    Command::new(prog)
}

#[derive(Debug)]
pub struct Saida {
    pub ok: bool,
    pub out: String,
    pub err: String,
}

/// Roda e espera, com teto. Devolve stdout, stderr e se saiu com 0.
///
/// Nao trata codigo != 0 como erro: varios passos aqui querem **ler** a
/// saida de um comando que falha (o `codesign` de um binario ad-hoc, o
/// `curl` de um 404) e decidir depois.
pub fn rodar(prog: &str, args: &[&str], segundos: u64) -> Result<Saida, String> {
    rodar_com_entrada(prog, args, segundos, None)
}

/// O mesmo, mas escreve algo no stdin do programa antes de esperar.
///
/// Existe por um motivo so: o token do GitHub entra no `curl` por
/// `-K -`, nunca em `argv`. Em `argv` ele apareceria no `ps` de qualquer
/// processo da maquina e nos logs de quem der `ps aux` num print.
pub fn rodar_com_entrada(
    prog: &str,
    args: &[&str],
    segundos: u64,
    entrada: Option<&str>,
) -> Result<Saida, String> {
    let mut filho = comando(prog)
        .args(args)
        .stdin(if entrada.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;

    if let Some(txt) = entrada {
        if let Some(mut si) = filho.stdin.take() {
            let _ = si.write_all(txt.as_bytes());
            // Fechar: `curl -K -` le a config ate o EOF.
        }
    }

    let limite = Instant::now() + Duration::from_secs(segundos);
    loop {
        match filho.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < limite => std::thread::sleep(Duration::from_millis(80)),
            Ok(None) => {
                let _ = filho.kill();
                let _ = filho.wait();
                return Err(format!("{prog} passou de {segundos}s e foi morto"));
            }
            Err(e) => return Err(format!("{prog}: {e}")),
        }
    }

    let s = filho.wait_with_output().map_err(|e| format!("{prog}: {e}"))?;
    Ok(Saida {
        ok: s.status.success(),
        out: String::from_utf8_lossy(&s.stdout).into_owned(),
        err: String::from_utf8_lossy(&s.stderr).into_owned(),
    })
}

/// `curl`, e nao uma pilha TLS nova em tres toolchains.
///
/// `curl` existe nas tres maquinas (o `probe.rs` do daemon ja o chama, e
/// no Windows por `curl.exe`, porque `curl` ali e apelido do
/// `Invoke-WebRequest`). Trazer `reqwest`/`rustls` para baixar um
/// `.tar.gz` por hora significaria um `ring` cruzado para mingw e uma
/// cadeia de certificados nossa, para ficar com a mesma garantia que o
/// sistema ja da.
fn curl_prog() -> &'static str {
    if cfg!(windows) {
        "curl.exe"
    } else {
        "curl"
    }
}

/// Config do `curl` com o token, para entrar por stdin.
fn config_do_token(token: Option<&str>) -> Option<String> {
    let t = token?.trim().to_string();
    if t.is_empty() {
        return None;
    }
    Some(format!("header = \"Authorization: Bearer {t}\"\n"))
}

/// Busca um recurso de texto (a resposta JSON da API).
pub fn api(url: &str, token: Option<&str>, segundos: u64) -> Result<String, String> {
    let cfg = config_do_token(token);
    let s = rodar_com_entrada(
        curl_prog(),
        &[
            "-sS",
            "-L",
            "--max-time",
            &segundos.to_string(),
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "X-GitHub-Api-Version: 2022-11-28",
            "-K",
            "-",
            url,
        ],
        segundos + 10,
        cfg.as_deref(),
    )?;
    if !s.ok {
        // Sem `-f`: o corpo de um 404 do GitHub diz mais que "HTTP 404".
        let motivo = s.err.lines().next().unwrap_or("").trim();
        let corpo = s.out.trim();
        return Err(format!(
            "curl falhou ao buscar {url}{}{}",
            if motivo.is_empty() { String::new() } else { format!(": {motivo}") },
            if corpo.is_empty() { String::new() } else { format!(" (corpo: {})", &corpo[..corpo.len().min(200)]) }
        ));
    }
    Ok(s.out)
}

/// Baixa um ativo da Release para um arquivo.
///
/// `Accept: application/octet-stream` no endereco da API: em repo
/// privado o `browser_download_url` da 404 mesmo com token.
pub fn baixar(url: &str, token: Option<&str>, destino: &Path, segundos: u64) -> Result<u64, String> {
    let cfg = config_do_token(token);
    let alvo = destino.to_string_lossy().into_owned();
    let s = rodar_com_entrada(
        curl_prog(),
        &[
            "-fsS",
            "-L",
            "--max-time",
            &segundos.to_string(),
            "-H",
            "Accept: application/octet-stream",
            "-o",
            &alvo,
            "-K",
            "-",
            url,
        ],
        segundos + 15,
        cfg.as_deref(),
    )?;
    if !s.ok {
        let _ = std::fs::remove_file(destino);
        let motivo = s.err.lines().next().unwrap_or("falhou sem mensagem").trim();
        return Err(format!("nao baixei {url}: {motivo}"));
    }
    let bytes = std::fs::metadata(destino).map(|m| m.len()).unwrap_or(0);
    if bytes == 0 {
        return Err(format!("baixei 0 bytes de {url}"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod testes {
    use super::*;

    /// O token nunca entra em `argv`. Este teste guarda a forma do que
    /// vai para o stdin do curl -- se alguem "simplificar" para `-H`, a
    /// config deixa de existir e o teste cai.
    #[test]
    fn o_token_vira_config_de_stdin() {
        let c = config_do_token(Some("ghp_exemplo")).unwrap();
        assert_eq!(c, "header = \"Authorization: Bearer ghp_exemplo\"\n");
        assert!(config_do_token(None).is_none());
        assert!(config_do_token(Some("   ")).is_none(), "token em branco nao vira header");
        // Quebra de linha do fim do arquivo nao entra no header.
        assert_eq!(config_do_token(Some("abc\n")).unwrap(), "header = \"Authorization: Bearer abc\"\n");
    }

    #[test]
    fn roda_e_traz_as_duas_saidas() {
        #[cfg(unix)]
        {
            let s = rodar("sh", &["-c", "echo saida; echo erro >&2; exit 3"], 5).unwrap();
            assert!(!s.ok);
            assert!(s.out.contains("saida"));
            assert!(s.err.contains("erro"));
        }
    }

    /// Teto de tempo e obrigatorio: isto roda sozinho, por tarefa
    /// agendada, sem ninguem para matar um comando pendurado.
    #[test]
    fn o_teto_mata_quem_pendura() {
        #[cfg(unix)]
        {
            let e = rodar("sh", &["-c", "sleep 30"], 1).unwrap_err();
            assert!(e.contains("foi morto"), "{e}");
        }
    }
}
