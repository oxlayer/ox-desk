//! O caminho inteiro: conferir, baixar, conferir, assinar, trocar,
//! reiniciar, conferir de novo, e voltar atras se nao responder.
//!
//! Spec: ../../specs/015-ota.md
//!
//! A ordem nao e arbitraria. **Assinar e testar acontecem na pasta de
//! passagem, antes de qualquer troca**: um binario do Mac que nao
//! executa (o `logistream-host` liga no ffmpeg do brew, e a CI compila
//! contra a versao do runner) tem de ser recusado *antes* de substituir
//! o que funciona, nao depois. Trocar e descobrir e o que exige
//! reverter; conferir antes e o que evita precisar.

use std::fs;
use std::path::{Path, PathBuf};

use crate::estado::{self, Estado};
use crate::exec;
use crate::lancamento::{self, Decisao};
use crate::plano::{self, Plano, Sistema};
use crate::sha256;

/// O repo de onde vem o build. Trocavel por ambiente para testar contra
/// outro repo sem recompilar.
const REPO: &str = "oxlayer/ox-desk";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modo {
    /// Conferir e relatar, sem tocar em binario nenhum.
    Conferir,
    /// Conferir e, se houver novidade, aplicar.
    Aplicar,
    /// Aplicar mesmo que o carimbo seja o mesmo (reinstalar).
    Forcar,
}

pub struct Contexto {
    pub sistema: Sistema,
    pub plano: Plano,
    pub base: PathBuf,
    pub dir: PathBuf,
    pub destino: PathBuf,
    pub modo: Modo,
    pub repo: String,
    pub token: Option<String>,
}

fn base() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let v = std::env::var_os("LOCALAPPDATA");
    #[cfg(not(windows))]
    let v = std::env::var_os("HOME");
    v.map(PathBuf::from).ok_or_else(|| "nao sei onde e a casa desta maquina".to_string())
}

/// O uid, tirado do dono da propria casa.
///
/// `launchctl kickstart gui/<uid>/...` precisa dele. Sai do dono de
/// `$HOME` em vez de `libc::getuid` para nao trazer o `libc` como
/// dependencia por um numero.
fn uid_de(casa: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return fs::metadata(casa).map(|m| m.uid()).unwrap_or(0);
    }
    #[cfg(not(unix))]
    {
        let _ = casa;
        0
    }
}

fn ler_token(s: Sistema, base: &Path) -> Option<String> {
    if let Ok(t) = std::env::var("LOGI_OTA_TOKEN") {
        if !t.trim().is_empty() {
            return Some(t);
        }
    }
    let p = plano::token_path(s, base);
    fs::read_to_string(p).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

pub fn contexto(modo: Modo) -> Result<Contexto, String> {
    let sistema = plano::daqui();
    let base = base()?;
    let plano = plano::plano(sistema, uid_de(&base));
    let dir = plano::estado_dir(sistema, &base);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let token = ler_token(sistema, &base);
    Ok(Contexto {
        sistema,
        destino: plano::destino(sistema, &base),
        plano,
        base,
        dir,
        modo,
        repo: std::env::var("LOGI_OTA_REPO").unwrap_or_else(|_| REPO.to_string()),
        token,
    })
}

fn nome_do_sistema(s: Sistema) -> &'static str {
    match s {
        Sistema::Linux => "linux",
        Sistema::Mac => "mac",
        Sistema::Windows => "windows",
    }
}

/// O que esta rodando aqui, agora.
///
/// Primeiro o `/healthz` do daemon vivo, que e a unica fonte que nao
/// mente: o arquivo no disco pode ser novo e o processo velho. Se o
/// daemon esta fora do ar, cai no ultimo estado gravado -- e se nem isso
/// existe, assume `0`, o que faz o OTA instalar o que houver. Isso e
/// deliberado: daemon morto com build desconhecido e exatamente o caso
/// em que atualizar e a saida.
fn aqui(ctx: &Contexto) -> (u64, String) {
    let url = format!("http://127.0.0.1:{}/healthz", porta(&ctx.base, ctx.sistema));
    if let Ok(txt) = exec::api(&url, None, 5) {
        if let Some((n, b)) = lancamento::ler_healthz(&txt) {
            return (n, b);
        }
    }
    match estado::ler(&ctx.dir) {
        Some(e) if e.n_aqui > 0 => (e.n_aqui, e.build_aqui),
        _ => (0, "?".to_string()),
    }
}

/// Conferir, e aplicar quando for o caso. Grava o estado sempre --
/// inclusive quando falha, que e justamente quando alguem precisa ler.
pub fn rodar(modo: Modo) -> Result<Estado, Estado> {
    let ctx = match contexto(modo) {
        Ok(c) => c,
        Err(e) => {
            // Sem contexto nao ha onde gravar. Devolve o erro nu.
            let mut est = Estado::novo("?", 0, "?");
            est.mensagem = e;
            return Err(est);
        }
    };

    let (n_aqui, build_aqui) = aqui(&ctx);
    let mut est = Estado::novo(nome_do_sistema(ctx.sistema), n_aqui, &build_aqui);

    let r = passos(&ctx, &mut est);
    match r {
        Ok(()) => {
            let _ = estado::escrever(&ctx.dir, &est);
            Ok(est)
        }
        Err(msg) => {
            if est.resultado == "erro" {
                est.mensagem = msg;
            }
            est.quando_unix = estado::agora();
            let _ = estado::escrever(&ctx.dir, &est);
            Err(est)
        }
    }
}

fn passos(ctx: &Contexto, est: &mut Estado) -> Result<(), String> {
    // `LOGI_OTA_API` existe para poder exercitar o caminho inteiro --
    // baixar, conferir a soma, extrair, trocar, reiniciar, conferir o
    // /healthz, reverter -- contra um servidor local, sem publicar uma
    // Release de mentira e sem mexer na unica fonte de verdade que as
    // tres maquinas consomem. Foi assim que a reversao foi testada.
    let api = std::env::var("LOGI_OTA_API")
        .unwrap_or_else(|_| "https://api.github.com".to_string());
    let url = format!("{api}/repos/{}/releases/latest", ctx.repo);
    let json = exec::api(&url, ctx.token.as_deref(), 30).map_err(|e| {
        if ctx.token.is_none() {
            format!("{e} -- e nao ha token em {}", plano::token_path(ctx.sistema, &ctx.base).display())
        } else {
            e
        }
    })?;
    let l = lancamento::ler(&json).map_err(|e| {
        if ctx.token.is_none() {
            format!("{e} (repo privado sem token: ponha um PAT de leitura em {})",
                plano::token_path(ctx.sistema, &ctx.base).display())
        } else {
            e
        }
    })?;
    est.tag = l.tag.clone();

    let ativo = lancamento::escolher(&l, ctx.sistema)?.clone();
    est.pacote = ativo.nome.clone();

    let decisao = lancamento::decidir(est.n_aqui, &est.build_aqui, &ativo.nome);
    if let Decisao::Atualizar { n, hash } | Decisao::Igual { n, hash } = &decisao {
        est.n_la = *n;
        est.build_la = hash.clone();
    }

    match decisao {
        Decisao::SemCarimbo { nome } => {
            return Err(format!("o pacote {nome} nao traz -<n>-<hash>; nao troco as cegas"));
        }
        Decisao::MaisNovoAqui { aqui, la } => {
            est.n_la = la;
            est.resultado = "mais-novo-aqui".into();
            est.mensagem = format!("#{aqui} aqui, #{la} publicado; nao desco de versao");
            return Ok(());
        }
        Decisao::Igual { .. } if ctx.modo != Modo::Forcar => {
            est.resultado = "igual".into();
            est.mensagem = format!("a Release {} e o que ja roda aqui", l.tag);
            return Ok(());
        }
        _ => {}
    }

    if ctx.modo == Modo::Conferir {
        est.resultado = "novidade".into();
        est.mensagem = "so conferi (--conferir)".into();
        return Ok(());
    }
    if let Some(motivo) = ctx.plano.recusa {
        est.resultado = "novidade".into();
        est.mensagem = motivo.to_string();
        return Ok(());
    }

    let somas = lancamento::somas(&l)?.clone();
    aplicar(ctx, est, &ativo, &somas)
}

// ---------------------------------------------------------------- troca

fn aplicar(
    ctx: &Contexto,
    est: &mut Estado,
    ativo: &lancamento::Ativo,
    somas: &lancamento::Ativo,
) -> Result<(), String> {
    // Pasta de passagem SEMPRE vazia: um download interrompido deixa
    // meio arquivo para tras, e meio `.tar.gz` com o nome certo e o
    // tipo de coisa que passa por "ja baixei".
    let passagem = ctx.dir.join("passagem");
    let _ = fs::remove_dir_all(&passagem);
    fs::create_dir_all(&passagem).map_err(|e| format!("{}: {e}", passagem.display()))?;

    let p_somas = passagem.join("SHA256SUMS");
    exec::baixar(&somas.url, ctx.token.as_deref(), &p_somas, 60)?;
    let txt_somas = fs::read_to_string(&p_somas).map_err(|e| format!("SHA256SUMS: {e}"))?;
    let esperada = lancamento::soma_de(&txt_somas, &ativo.nome)
        .ok_or_else(|| format!("o SHA256SUMS da Release nao tem linha para {}", ativo.nome))?;

    let p_tar = passagem.join(&ativo.nome);
    let bytes = exec::baixar(&ativo.url, ctx.token.as_deref(), &p_tar, 300)?;

    let dados = fs::read(&p_tar).map_err(|e| format!("{}: {e}", p_tar.display()))?;
    let obtida = sha256::hex(&dados);
    if obtida != esperada {
        return Err(format!(
            "o pacote nao confere: esperava {esperada}, baixei {obtida} ({bytes} bytes)"
        ));
    }
    drop(dados);

    // `tar` do sistema: existe nas tres maquinas (o Windows 10 traz
    // `tar.exe` desde 1803, e o clip_arq.rs ja o usa nas tres).
    let s = exec::rodar(
        "tar",
        &["-xzf", &p_tar.to_string_lossy(), "-C", &passagem.to_string_lossy()],
        120,
    )?;
    if !s.ok {
        return Err(format!("tar falhou: {}", primeira_linha(&s.err)));
    }

    let dentro = passagem.join(ativo.nome.trim_end_matches(".tar.gz"));
    if !dentro.is_dir() {
        return Err(format!("o pacote nao trouxe a pasta {}", dentro.display()));
    }

    let sums_internas = fs::read_to_string(dentro.join("SHA256SUMS"))
        .map_err(|e| format!("o pacote nao traz SHA256SUMS interno: {e}"))?;
    let itens = lancamento::linhas(&sums_internas);
    if itens.is_empty() {
        return Err("o SHA256SUMS interno nao lista nada".into());
    }

    for (soma, nome) in &itens {
        let f = dentro.join(nome);
        let d = fs::read(&f).map_err(|e| format!("{nome}: {e}"))?;
        let obtida = sha256::hex(&d);
        if &obtida != soma {
            return Err(format!("{nome} nao confere: esperava {soma}, veio {obtida}"));
        }
    }

    // O carimbo prova que este binario e DESTE commit -- a mesma
    // pergunta que o `construir-binarios.sh` faz antes de empacotar e
    // que o `deploy.sh` faz ao /healthz no fim.
    let daemon = dentro.join(ctx.plano.daemon);
    let cru = fs::read(&daemon).map_err(|e| format!("{}: {e}", ctx.plano.daemon))?;
    if !contem(&cru, est.build_la.as_bytes()) {
        return Err(format!(
            "o {} do pacote nao carrega o carimbo {}",
            ctx.plano.daemon, est.build_la
        ));
    }
    drop(cru);

    // Assinar ANTES de instalar e de testar: no Mac e a assinatura que
    // mantem o *designated requirement* e, com ele, a concessao de
    // Gravacao de Tela. Instalar primeiro e assinar depois deixaria uma
    // janela em que o binario instalado e ad-hoc.
    if ctx.plano.assina {
        assinar_tudo(ctx, &dentro, &itens)?;
    }

    // Testar na passagem. Binario que nao carrega e recusado aqui, com
    // o que funciona ainda no lugar.
    for (_, nome) in &itens {
        conferir_ligacoes(ctx.sistema, &dentro.join(nome))?;
    }
    fumaca_do_daemon(&daemon)?;

    // --- daqui para baixo a maquina muda. Backup primeiro.
    let anterior = ctx.dir.join("anterior");
    let _ = fs::remove_dir_all(&anterior);
    fs::create_dir_all(&anterior).map_err(|e| format!("{}: {e}", anterior.display()))?;
    let mut salvos: Vec<String> = Vec::new();
    for (_, nome) in &itens {
        let atual = ctx.destino.join(nome);
        if atual.is_file() {
            copiar(&atual, &anterior.join(nome))?;
            salvos.push(nome.clone());
        }
    }
    fs::write(anterior.join("build"), format!("{} {}\n", est.n_aqui, est.build_aqui)).ok();

    for (_, nome) in &itens {
        copiar(&dentro.join(nome), &ctx.destino.join(nome))?;
    }

    let reinicio = reiniciar(ctx);
    let subiu = reinicio.is_ok() && conferir_healthz(&ctx.base, ctx.sistema, &est.build_la).is_ok();

    if subiu {
        est.resultado = "atualizado".into();
        est.mensagem = format!(
            "{} binario(s) trocado(s) e /healthz respondeu {}",
            itens.len(),
            est.build_la
        );
        est.n_aqui = est.n_la;
        est.build_aqui = est.build_la.clone();
        let _ = fs::remove_dir_all(&passagem);
        return Ok(());
    }

    // Reverter. Se nem isto funcionar, a mensagem tem de dizer onde a
    // copia ficou -- e o que alguem vai precisar as 2 da manha.
    let por_que = match reinicio {
        Err(e) => format!("o reinicio falhou: {e}"),
        Ok(()) => match conferir_healthz(&ctx.base, ctx.sistema, &est.build_la) {
            Err(e) => e,
            Ok(()) => "sem motivo claro".into(),
        },
    };
    let mut voltou = true;
    for nome in &salvos {
        if copiar(&anterior.join(nome), &ctx.destino.join(nome)).is_err() {
            voltou = false;
        }
    }
    let _ = reiniciar(ctx);
    let de_volta = conferir_healthz(&ctx.base, ctx.sistema, &est.build_aqui).is_ok();

    est.resultado = "revertido".into();
    est.mensagem = if voltou && de_volta {
        format!("{por_que}; voltei ao build anterior e ele responde")
    } else {
        format!(
            "{por_que}; a volta NAO confirmou -- a copia boa esta em {}",
            anterior.display()
        )
    };
    Err(est.mensagem.clone())
}

fn primeira_linha(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("sem mensagem").to_string()
}

fn contem(agulheiro: &[u8], agulha: &[u8]) -> bool {
    if agulha.is_empty() || agulha.len() > agulheiro.len() {
        return false;
    }
    agulheiro.windows(agulha.len()).any(|j| j == agulha)
}

/// Copia preservando o bit de execucao, e **apagando o destino antes**.
///
/// Escrever por cima de um binario em execucao falha com `ETXTBSY` no
/// Linux. Apagar primeiro funciona porque quem esta rodando segue com o
/// inode antigo aberto -- e e o que faz o reinicio pegar o arquivo novo.
fn copiar(de: &Path, para: &Path) -> Result<(), String> {
    if let Some(p) = para.parent() {
        fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let _ = fs::remove_file(para);
    fs::copy(de, para).map_err(|e| format!("{} -> {}: {e}", de.display(), para.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(para, fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

fn assinar_tudo(ctx: &Contexto, dentro: &Path, itens: &[(String, String)]) -> Result<(), String> {
    let script = plano::assinador(ctx.sistema, &ctx.base)
        .ok_or_else(|| "este sistema nao tem assinador".to_string())?;
    if !script.is_file() {
        return Err(format!(
            "{} nao existe; sem ele o binario entra ad-hoc e o Mac perde a Gravacao de Tela",
            script.display()
        ));
    }
    for (_, nome) in itens {
        let alvo = dentro.join(nome);
        let s = exec::rodar(&script.to_string_lossy(), &[&alvo.to_string_lossy()], 90)?;
        if !s.ok {
            return Err(format!("assinar {nome}: {}", primeira_linha(&s.err)));
        }
        // O `assinar.sh` e deliberadamente nao-fatal (ele nao derruba
        // nada por conta propria). Quem transforma em falha e quem
        // confere -- e aqui a falha importa: binario ad-hoc instalado
        // significa tela preta no proximo pedido.
        let v = exec::rodar("codesign", &["-dvvv", &alvo.to_string_lossy()], 30)?;
        let tudo = format!("{}{}", v.out, v.err);
        if tudo.contains("adhoc") || !tudo.contains("Authority=") {
            return Err(format!("{nome} seguiu sem identidade apos assinar; rode criar-identidade.sh no Mac"));
        }
    }
    Ok(())
}

/// O binario acha as bibliotecas de que depende, nesta maquina?
///
/// E o risco real e conhecido do pacote do Mac: o `logistream-host` e o
/// `-view` ligam no ffmpeg do brew, a CI compila contra a versao do
/// runner, e major diferente nao executa. Conferir a ligacao em vez de
/// executar: alguns desses binarios abrem janela, e uma janela abrindo
/// sozinha no meio da sessao de alguem e pior que o problema.
fn conferir_ligacoes(s: Sistema, bin: &Path) -> Result<(), String> {
    let nome = bin.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match s {
        Sistema::Mac => {
            let r = exec::rodar("otool", &["-L", &bin.to_string_lossy()], 30)?;
            let faltando: Vec<String> = r
                .out
                .lines()
                .map(str::trim)
                .filter_map(|l| l.split(" (").next())
                .filter(|p| p.starts_with("/opt/") || p.starts_with("/usr/local/"))
                .filter(|p| !Path::new(p).exists())
                .map(|p| p.to_string())
                .collect();
            if !faltando.is_empty() {
                return Err(format!(
                    "{nome} precisa de {} e nao esta aqui (ffmpeg do brew em versao diferente da do runner?)",
                    faltando.join(", ")
                ));
            }
            Ok(())
        }
        Sistema::Linux => {
            let r = exec::rodar("ldd", &[&bin.to_string_lossy()], 30)?;
            let faltando: Vec<&str> =
                r.out.lines().filter(|l| l.contains("not found")).map(str::trim).collect();
            if !faltando.is_empty() {
                return Err(format!("{nome}: biblioteca ausente -- {}", faltando.join("; ")));
            }
            Ok(())
        }
        Sistema::Windows => Ok(()),
    }
}

/// O daemon do pacote executa?
///
/// `--help` nao abre porta e nao carrega config (ver `run_cli` em
/// `logid/src/main.rs`), entao e seguro rodar com o daemon de verdade no
/// ar. O que se procura e erro de **carregador**, nao codigo de saida:
/// a ajuda sai pelo stderr de proposito, e sair com != 0 nao e sintoma.
fn fumaca_do_daemon(bin: &Path) -> Result<(), String> {
    let r = exec::rodar(&bin.to_string_lossy(), &["--help"], 20)
        .map_err(|e| format!("o daemon do pacote nao executou: {e}"))?;
    let tudo = format!("{}{}", r.out, r.err);
    for sinal in [
        "error while loading shared libraries",
        "Library not loaded",
        "Symbol not found",
        "dyld",
        "cannot execute",
        "Exec format error",
    ] {
        if tudo.contains(sinal) {
            return Err(format!("o daemon do pacote nao carrega: {}", primeira_linha(&tudo)));
        }
    }
    Ok(())
}

fn reiniciar(ctx: &Contexto) -> Result<(), String> {
    let argv = &ctx.plano.reiniciar;
    let (prog, args) = argv.split_first().ok_or_else(|| "sem comando de reinicio".to_string())?;
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let s = exec::rodar(prog, &args, 60)?;
    if !s.ok {
        return Err(format!("{prog}: {}", primeira_linha(&s.err)));
    }
    Ok(())
}

/// O daemon vivo e o build que acabamos de instalar?
///
/// Tenta por ~25 s: `systemctl restart` devolve antes de o processo
/// estar servindo, e o `launchctl kickstart` tambem. Perguntar uma vez
/// so reprovaria um update que estava certo.
fn conferir_healthz(base: &Path, s: Sistema, esperado: &str) -> Result<(), String> {
    let url = format!("http://127.0.0.1:{}/healthz", porta(base, s));
    let mut ultimo = String::from("nao respondeu");
    for _ in 0..13 {
        std::thread::sleep(std::time::Duration::from_millis(2000));
        match exec::api(&url, None, 4) {
            Ok(t) => match lancamento::ler_healthz(&t) {
                Some((_, b)) if b == esperado => return Ok(()),
                Some((n, b)) => ultimo = format!("/healthz responde #{n} {b}, esperava {esperado}"),
                None => ultimo = "o /healthz respondeu algo que nao sei ler".into(),
            },
            Err(e) => ultimo = e,
        }
    }
    Err(format!("o /healthz nao confirmou {esperado} em ~26 s: {ultimo}"))
}

/// A porta do daemon, lida da config desta maquina.
///
/// Chumbar 8787 faria o OTA achar que o daemon esta morto numa maquina
/// que so mudou de porta -- e daemon "morto" e justamente o caso em que
/// ele instala sem conferir nada. Le a linha `port = N` em vez de trazer
/// o `toml` como dependencia por um numero.
fn porta(base: &Path, s: Sistema) -> u16 {
    if let Some(p) = std::env::var("LOGI_PORT").ok().and_then(|p| p.parse::<u16>().ok()) {
        return p;
    }
    // No Windows a config NAO fica junto dos binarios: o `logid` a le de
    // `%APPDATA%\logi` (Roaming), e os binarios vivem em
    // `%LOCALAPPDATA%\logid`. Procurar no mesmo lugar dos dois seria
    // nunca achar -- e cair no 8787 chumbado, que e o bug que esta
    // funcao existe para evitar. Conferido na saida do wizard de la:
    // `C:\Users\user\AppData\Roaming\logi\config.toml`.
    let cfg = match s {
        Sistema::Linux | Sistema::Mac => base.join(".config").join("logi").join("config.toml"),
        Sistema::Windows => std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| base.to_path_buf())
            .join("logi")
            .join("config.toml"),
    };
    fs::read_to_string(cfg)
        .ok()
        .and_then(|t| {
            t.lines()
                .map(str::trim)
                // Parar na primeira tabela. A config tem `port = 22`
                // dentro de cada `[[hosts]]` (a porta do SSH), e uma
                // varredura ingenua pegaria 22 em qualquer maquina que
                // omitisse a porta do daemon -- fazendo o OTA concluir
                // "daemon morto" e instalar sem conferir nada.
                .take_while(|l| !l.starts_with('['))
                .find(|l| l.starts_with("port"))
                .and_then(|l| l.split('=').nth(1))
                .and_then(|v| v.trim().trim_matches('"').parse::<u16>().ok())
        })
        .unwrap_or(8787)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn acha_o_carimbo_dentro_de_bytes_quaisquer() {
        let bin = b"\x7fELF\x02\x01...logid 2f1409d em http://127.0.0.1\x00mais lixo";
        assert!(contem(bin, b"2f1409d"));
        assert!(!contem(bin, b"deadbee"));
        assert!(!contem(bin, b""), "agulha vazia nao casa nada");
        assert!(!contem(b"ab", b"abc"), "agulha maior que o palheiro");
    }

    /// A pasta extraida e o nome do pacote sem `.tar.gz` -- e como o
    /// `construir-binarios.sh` empacota (`tar -C dist <basename>`).
    #[test]
    fn a_pasta_do_pacote_e_o_nome_sem_extensao() {
        let nome = "logi-mac-arm64-unsigned-124-abc1234.tar.gz";
        assert_eq!(nome.trim_end_matches(".tar.gz"), "logi-mac-arm64-unsigned-124-abc1234");
    }

    #[test]
    fn copiar_apaga_o_destino_antes() {
        let d = std::env::temp_dir().join(format!("logi-ota-cp-{}", estado::agora()));
        fs::create_dir_all(&d).unwrap();
        let a = d.join("a");
        let b = d.join("b");
        fs::write(&a, "novo").unwrap();
        fs::write(&b, "velho e mais longo").unwrap();
        copiar(&a, &b).unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "novo");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&b).unwrap().permissions().mode() & 0o777, 0o755);
        }
        fs::remove_dir_all(&d).ok();
    }

    /// A porta sai da config, e config ausente cai em 8787 -- nunca em
    /// zero nem em erro, porque isto roda sem ninguem olhando.
    #[test]
    fn a_porta_sai_da_config() {
        let d = std::env::temp_dir().join(format!("logi-ota-porta-{}", estado::agora()));
        let cfg = d.join(".config").join("logi");
        fs::create_dir_all(&cfg).unwrap();
        fs::write(cfg.join("config.toml"), "# comentario\nport = 9191\nfollow = true\n").unwrap();
        assert_eq!(porta(&d, Sistema::Linux), 9191);
        fs::write(cfg.join("config.toml"), "sem porta aqui\n").unwrap();
        assert_eq!(porta(&d, Sistema::Linux), 8787);
        // O `port = 22` do SSH de cada host nao pode virar a porta do
        // daemon quando a do daemon nao esta escrita.
        fs::write(
            cfg.join("config.toml"),
            "follow = true\n\n[[hosts]]\nname = \"windows\"\nport = 22\n",
        )
        .unwrap();
        assert_eq!(porta(&d, Sistema::Linux), 8787);
        assert_eq!(porta(Path::new("/nao/existe"), Sistema::Linux), 8787);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn primeira_linha_ignora_vazias() {
        assert_eq!(primeira_linha("\n\n  erro de verdade\noutra"), "erro de verdade");
        assert_eq!(primeira_linha(""), "sem mensagem");
    }
}
