//! Medições de setup que são iguais nas três máquinas.
//!
//! Spec: ../../specs/011-onboarding.md
//!
//! Existe para os passos cujo **critério** não muda de sistema -- a
//! config conhece esta máquina? o SSH alcança os pares? o clipboard está
//! de pé? -- ainda que o mecanismo por baixo mude. Onde o mecanismo
//! difere, quem difere é `crate::sys` ou `crate::remote`, que já
//! existem para isso.

use crate::config::Config;
use crate::setup::Medida;
use std::time::Duration;

/// Roda um programa com teto de tempo e devolve o stdout.
///
/// Teto porque isto roda dentro de uma requisição HTTP: um comando que
/// pendura (e no macOS vários penduram quando falta permissão, em vez de
/// falhar) travaria uma das quatro threads do servidor até alguém
/// reiniciar o daemon.
pub fn rodar(prog: &str, args: &[&str], segundos: u64) -> Result<String, String> {
    let mut filho = crate::sys::child(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;

    // `wait_timeout` seria uma dependência; isto é uma espera por
    // amostragem de 50 ms, que para um comando de setup é indistinguível.
    let limite = std::time::Instant::now() + Duration::from_secs(segundos);
    loop {
        match filho.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < limite => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = filho.kill();
                let _ = filho.wait();
                return Err(format!("{prog} passou de {segundos}s e foi morto"));
            }
            Err(e) => return Err(format!("{prog}: {e}")),
        }
    }
    let saida = filho.wait_with_output().map_err(|e| format!("{prog}: {e}"))?;
    let texto = String::from_utf8_lossy(&saida.stdout).into_owned();
    if saida.status.success() {
        return Ok(texto);
    }
    let err = String::from_utf8_lossy(&saida.stderr);
    let linha = err
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .or_else(|| texto.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("falhou sem mensagem");
    Err(linha.to_string())
}

/// A config conhece esta máquina, e por qual nome.
pub fn config_desta_maquina(cfg: &Config) -> Medida {
    let p = Config::path();
    let locais: Vec<&str> =
        cfg.hosts.iter().filter(|h| h.is_local()).map(|h| h.name.as_str()).collect();
    match locais.len() {
        1 => Medida::ok("config", format!("{} é esta máquina ({})", locais[0], p.display())),
        0 => Medida::falta("config", format!("nenhum host marcado como local em {}", p.display())),
        // Dois "locais" fazem o painel, a borda e o roteamento de input
        // discordarem sobre quem é "aqui", e cada um escolhe o primeiro
        // que acha -- que não é necessariamente o mesmo.
        n => Medida::falta("config", format!("{n} hosts marcados como local: {}", locais.join(", "))),
    }
}

/// O SSH alcança cada par, sem senha.
///
/// Mede em paralelo porque três hosts em série, com um fora do ar, são
/// três timeouts somados -- e o wizard mediria em 12 s o que mede em 4.
pub fn pares(cfg: &Config) -> Medida {
    let alvos: Vec<&crate::config::HostCfg> = cfg.hosts.iter().filter(|h| !h.is_local()).collect();
    if alvos.is_empty() {
        return Medida::fora("pares", "nenhum outro host na config");
    }
    let resultados: Vec<(String, Result<String, String>)> = std::thread::scope(|s| {
        let hs: Vec<_> = alvos
            .iter()
            .map(|h| s.spawn(move || (h.name.clone(), crate::remote::run(h, "echo ok", 4))))
            .collect();
        hs.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    let ruins: Vec<String> = resultados
        .iter()
        .filter(|(_, r)| !matches!(r, Ok(s) if s.contains("ok")))
        .map(|(n, r)| match r {
            Err(e) => format!("{n}: {e}"),
            // Respondeu, mas não o que pedimos: shell que imprime banner,
            // perfil que escreve na saída, PowerShell que não espera.
            Ok(s) => format!("{n}: respondeu \"{}\"", s.trim().replace('\n', " ")),
        })
        .collect();

    if ruins.is_empty() {
        Medida::ok("pares", format!("{} de {} alcançáveis", resultados.len(), alvos.len()))
    } else {
        Medida::falta("pares", ruins.join("; "))
    }
}

/// A vigilância do clipboard está de pé?
///
/// Lê a anotação que a própria vigilância deixa (`clip::vigia`), em vez
/// de testar de fora. Testar de fora exigiria escrever no clipboard da
/// pessoa -- e um wizard que apaga o que você copiou para provar que
/// funciona é pior que o problema.
pub fn clipboard() -> Medida {
    match crate::clip::vigia() {
        Some((true, d)) => Medida::ok("clipboard", d),
        Some((false, d)) => Medida::falta("clipboard", d),
        None => Medida::erro("clipboard", "a vigilância ainda não reportou nada"),
    }
}

// ------------------------------------------------------------------ reparos

/// Grava a config padrão, preservando a existente numa cópia.
pub fn escrever_config() -> Result<String, String> {
    let p = Config::path();
    if p.exists() {
        // Sobrescrever a config de alguém sem cópia é destrutivo e não
        // tem desfazer: aqui moram os canais Easy-Switch, a grade e os
        // apelidos de SSH, tudo escrito à mão.
        let bak = p.with_extension("toml.bak");
        std::fs::copy(&p, &bak).map_err(|e| format!("copiar para {}: {e}", bak.display()))?;
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("criar {}: {e}", dir.display()))?;
    }
    std::fs::write(&p, crate::config::DEFAULT_TOML)
        .map_err(|e| format!("gravar {}: {e}", p.display()))?;
    // Reabrir antes de dizer que deu certo: um default que não parseia
    // deixaria o daemon sem config no próximo start.
    Config::load().map_err(|e| format!("gravei e não reabriu: {e}"))?;
    Ok(format!("config padrão em {} (a anterior ficou em .toml.bak)", p.display()))
}

/// Força uma amostragem do clipboard e devolve o que ela mediu.
pub fn testar_clipboard() -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        // No Linux não há amostragem: o sinal do Mutter avisa na hora, e
        // a anotação já reflete a sessão. Reiniciar a thread do D-Bus
        // para "testar" perderia a posse da seleção atual -- o que a
        // pessoa copiou sumiria no próximo Ctrl+V.
        match crate::clip::vigia() {
            Some((true, d)) => Ok(d),
            Some((false, d)) => Err(format!("{d}. Reinicie o daemon para tentar de novo.")),
            None => Err("a thread do clipboard não reportou: veja o journal do logid".into()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Uma leitura agora, pelo mesmo caminho da vigilância: ela
        // anota, e nós lemos a anotação. Ler direto aqui mediria outro
        // caminho que não é o que roda de verdade.
        let _ = crate::clip_arq::ler_local();
        match crate::clip::vigia() {
            Some((true, d)) => Ok(d),
            Some((false, d)) => Err(d),
            None => Err("a leitura não reportou nada".into()),
        }
    }
}

/// `pgrep`/`tasklist` do nosso jeito: o processo existe?
pub fn processo_vivo(nome: &str) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("pgrep")
            .args(["-x", nome])
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let _ = nome;
        crate::sys::unit_state(nome) == "active"
    }
}
