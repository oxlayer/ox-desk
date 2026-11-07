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

// ---------------------------------------------------------------- OTA

/// "há 12 min", "há 3 h", "há 2 dias".
///
/// `saturating_add` nos arredondamentos: a idade pode chegar aqui como
/// `u64::MAX` quando o campo veio ilegível, e um `+ 43_200` cru derruba
/// o daemon em build de debug. Foi o teste que achou.
pub fn idade_humana(s: u64) -> String {
    match s {
        0..=89 => format!("há {s}s"),
        90..=5399 => format!("há {} min", s.saturating_add(30) / 60),
        5400..=172_799 => format!("há {} h", s.saturating_add(1800) / 3600),
        _ => format!("há {} dias", s.saturating_add(43_200) / 86_400),
    }
}

/// Como está a atualização automática desta máquina.
///
/// Não lê o `estado.json` do `logi-ota` nem reescreve as frases dele:
/// pergunta ao próprio binário (`--estado --curto`) e corta em TAB. O
/// `logid` e o `logi-ota` são binários separados -- duplicar aqui o
/// formato do arquivo seria a mesma armadilha de ter a lista de
/// binários em dois lugares ([[../../specs/013-ci-cd.md]]).
///
/// `relogio` é o que cada sistema mediu do *seu* agendador: timer do
/// systemd, `StartInterval` do launchd, tarefa agendada.
pub fn ota(bin: &std::path::Path, token: &std::path::Path, relogio: Result<String, String>) -> Medida {
    if !bin.is_file() {
        return Medida::falta("ota", format!("{} não está instalado; rode o deploy", bin.display()));
    }
    // Sem token nada funciona, e o sintoma seria um "Not Found" da API
    // que manda procurar na CI. Então a falta do token é o diagnóstico.
    if !token.is_file() {
        return Medida::falta(
            "ota",
            format!(
                "falta o token de leitura em {}: o repo é privado e a API responde \"Not Found\" sem ele",
                token.display()
            ),
        );
    }
    // Segredo legível por qualquer processo da máquina é segredo
    // vazado -- e o Mac é justamente o que sai de casa.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = std::fs::metadata(token) {
            let modo = m.permissions().mode() & 0o077;
            if modo != 0 {
                return Medida::falta(
                    "ota",
                    format!("{} está com modo {:o}: só o dono deve ler (chmod 600)", token.display(), m.permissions().mode() & 0o777),
                );
            }
        }
    }
    let relogio = match relogio {
        Ok(t) => t,
        Err(e) => return Medida::falta("ota", format!("o relógio não está de pé: {e}")),
    };

    let linha = match rodar(&bin.to_string_lossy(), &["--estado", "--curto"], 8) {
        Ok(l) => l.trim().to_string(),
        // Sai com != 0 quando nunca conferiu. É estado legítimo logo
        // depois de instalar, não erro.
        Err(_) => return Medida::falta("ota", format!("{relogio}, mas ainda não conferiu nenhuma vez")),
    };
    veredito(&linha, &relogio)
}

/// A decisão, separada de quem mede.
///
/// Pura porque é aqui que mora o critério -- e o critério tem um caso
/// que só se vê escrito: **última checagem bem-sucedida não basta**. Um
/// relógio de uma hora que não acorda há seis deixou de proteger, e
/// como o último resultado dele foi "em dia", mostrá-lo como `Ok` é
/// dizer que está tudo bem justamente quando parou de estar.
pub fn veredito(linha: &str, relogio: &str) -> Medida {
    let campos: Vec<&str> = linha.split('\t').collect();
    if campos.len() != 3 {
        return Medida::erro("ota", format!("não sei ler o estado do logi-ota: {linha:?}"));
    }
    let (resultado, idade, frase) =
        (campos[0], campos[1].parse::<u64>().unwrap_or(u64::MAX), campos[2]);
    let quando = idade_humana(idade);

    match resultado {
        "erro" | "revertido" => Medida::erro("ota", format!("{quando}: {frase}")),
        _ if idade > 6 * 3600 => Medida::falta(
            "ota",
            format!("{relogio}, mas a última checagem foi {quando} -- o relógio não está acordando"),
        ),
        // Novidade publicada e não aplicada é pendência, não sucesso:
        // no Windows é o estado permanente (não assinamos lá), e é
        // justamente isso que o painel precisa mostrar.
        "novidade" => Medida::falta("ota", format!("{quando}: {frase}")),
        _ => Medida::ok("ota", format!("{relogio}; {quando}: {frase}")),
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    /// A frase do painel é lida por humano: 5400 s é "1 h e meia", e
    /// mostrar "5400s" seria obrigar quem lê a dividir de cabeça.
    #[test]
    fn idade_em_palavras() {
        assert_eq!(idade_humana(0), "há 0s");
        assert_eq!(idade_humana(89), "há 89s");
        assert_eq!(idade_humana(90), "há 2 min");
        assert_eq!(idade_humana(3600), "há 60 min");
        assert_eq!(idade_humana(5400), "há 2 h");
        assert_eq!(idade_humana(90_000), "há 25 h");
        assert_eq!(idade_humana(172_800), "há 2 dias");
    }

    /// Binário ausente é a primeira pergunta, antes de token e relógio:
    /// sem ele nenhum dos outros dois tem sentido.
    #[test]
    fn sem_binario_nao_cobra_token() {
        let m = ota(
            std::path::Path::new("/nao/existe/logi-ota"),
            std::path::Path::new("/nao/existe/token"),
            Ok("relógio ok".into()),
        );
        assert_eq!(m.estado, crate::setup::Estado::Falta);
        assert!(m.detalhe.contains("não está instalado"), "{}", m.detalhe);
    }

    /// Relógio parado não pode aparecer como "ok" só porque a última
    /// checagem deu certo: é exatamente o caso em que a máquina para de
    /// se atualizar e nada avisa.
    #[test]
    fn relogio_parado_nao_e_ok_mesmo_com_resultado_bom() {
        let recente = veredito("igual\t600\tem dia no build #124 abc1234", "a cada 1 h");
        assert_eq!(recente.estado, crate::setup::Estado::Ok);

        let parado = veredito("igual\t25000\tem dia no build #124 abc1234", "a cada 1 h");
        assert_eq!(parado.estado, crate::setup::Estado::Falta);
        assert!(parado.detalhe.contains("não está acordando"), "{}", parado.detalhe);
    }

    #[test]
    fn erro_e_reversao_sao_erro_e_novidade_e_pendencia() {
        assert_eq!(veredito("erro\t60\tsem token", "x").estado, crate::setup::Estado::Erro);
        assert_eq!(veredito("revertido\t60\tvoltei", "x").estado, crate::setup::Estado::Erro);
        assert_eq!(veredito("novidade\t60\tha o #125", "x").estado, crate::setup::Estado::Falta);
        assert_eq!(veredito("atualizado\t60\ttroquei", "x").estado, crate::setup::Estado::Ok);
        assert_eq!(veredito("mais-novo-aqui\t60\t#130 aqui", "x").estado, crate::setup::Estado::Ok);
    }

    /// Linha fora do contrato é erro, não "ok" por omissão. Se o
    /// `logi-ota` mudar o formato, o painel diz isso em vez de mentir.
    #[test]
    fn linha_fora_do_contrato_e_erro() {
        let m = veredito("so um campo", "x");
        assert_eq!(m.estado, crate::setup::Estado::Erro);
        assert!(m.detalhe.contains("não sei ler"), "{}", m.detalhe);
    }

    /// Idade ilegível conta como velha, nunca como fresca.
    #[test]
    fn idade_ilegivel_nao_passa_por_fresca() {
        assert_eq!(veredito("igual\tabc\tfrase", "x").estado, crate::setup::Estado::Falta);
    }
}
