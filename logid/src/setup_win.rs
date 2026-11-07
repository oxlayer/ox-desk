//! Medição e reparo do setup, no Windows.
//!
//! Spec: ../../specs/011-onboarding.md
//!
//! Quase tudo aqui gira em volta de **uma** fronteira: a sessão. Serviço
//! nasce na sessão 0, e de lá não se vê desktop para duplicar, não se
//! alcança `SendInput` e a área de transferência está sempre vazia. Por
//! isso o daemon e o host de tela sobem por tarefa agendada com `/IT`, e
//! por isso o wizard não se contenta em ver que a tarefa existe --
//! confere o `LogonType` dela.
//!
//! ## Por que XML e não `schtasks /query /v`
//!
//! A saída tabular do `schtasks` é **traduzida**: num Windows pt-BR o
//! campo é "Modo de logon: Somente interativo". Procurar "Interactive"
//! ali funcionaria nesta máquina e falharia na próxima. O `/xml` devolve
//! `<LogonType>InteractiveToken</LogonType>`, que é nome de esquema e
//! não muda com o idioma. Mesma lição que `unit_state` aprendeu lendo
//! "ESTADO" em vez de "STATE".

use crate::config::Config;
use crate::setup::Medida;
use crate::setup_comum::rodar;
use std::path::PathBuf;

const BINS: [&str; 5] = [
    "logid.exe",
    "logid-agent.exe",
    "logistream-host.exe",
    "logistream-view.exe",
    "logi-ota.exe",
];

fn bin_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
    PathBuf::from(base).join("logid")
}

pub fn medir(cfg: &Config) -> Vec<Medida> {
    vec![
        binarios(),
        crate::setup_comum::config_desta_maquina(cfg),
        tarefa("daemon", "logid"),
        tarefa("tarefa-tela", "logistream-host"),
        crate::setup_comum::clipboard(),
        sshd(),
        chave_admin(),
        crate::setup_comum::pares(cfg),
        protetor_de_tela(),
        ota(),
    ]
}

fn token_ota() -> PathBuf {
    bin_dir().join("ota-token")
}

fn ota() -> Medida {
    crate::setup_comum::ota(&bin_dir().join("logi-ota.exe"), &token_ota(), relogio())
}

/// A tarefa agendada que acorda o `logi-ota`.
fn relogio() -> Result<String, String> {
    let m = tarefa("ota", "logi-ota");
    if m.estado == crate::setup::Estado::Ok {
        Ok(m.detalhe)
    } else {
        Err(m.detalhe)
    }
}

fn binarios() -> Medida {
    let dir = bin_dir();
    let faltam: Vec<&str> = BINS.iter().copied().filter(|b| !dir.join(b).exists()).collect();
    if !faltam.is_empty() {
        return Medida::falta("binarios", format!("faltam: {}", faltam.join(", ")));
    }
    // Existir no disco não é estar no ar. Ver
    // `setup_comum::veredito_do_carimbo`: um deploy pode instalar o
    // arquivo novo e ninguém executá-lo.
    if let Some(p) = crate::setup_comum::daemon_do_disco(&dir.join("logid.exe")) {
        return Medida::falta("binarios", p);
    }
    Medida::ok("binarios", format!("{} binários em {}", BINS.len(), dir.display()))
}

/// A tarefa existe E roda na sessão interativa?
fn tarefa(id: &str, nome: &str) -> Medida {
    // Falhar aqui é "não existe", e não "não sei".
    //
    // A primeira versão tentava distinguir pelo TEXTO do erro
    // ("annot find" / "ão foi poss") e eu teria descoberto tarde que
    // este Windows é pt-BR: a mensagem real é "ERRO: O sistema não pode
    // encontrar o arquivo especificado", e a acentuação ainda chega
    // escapada pelo caminho do SSH. Tratar qualquer falha como ausência
    // é certo na prática -- `schtasks /query` numa tarefa que existe
    // não falha -- e a mensagem do SO vai junto para o caso raro.
    let xml = match rodar("schtasks", &["/query", "/tn", nome, "/xml", "ONE"], 10) {
        Ok(x) => x,
        Err(e) => return Medida::falta(id, format!("não achei a tarefa \"{nome}\" ({e})")),
    };
    // O `/xml` sai em UTF-16 e o `rodar` o lê como UTF-8 lossy, então o
    // texto vem salpicado de NULs. Tirar os zeros é mais barato (e mais
    // robusto) que decodificar: o que queremos é uma busca de substring.
    let limpo: String = xml.chars().filter(|c| *c != '\0').collect();
    if !limpo.contains("InteractiveToken") {
        return Medida::falta(
            id,
            format!("\"{nome}\" existe mas NÃO é interativa: cairia na sessão 0"),
        );
    }
    Medida::ok(id, format!("\"{nome}\" existe e roda na sessão interativa"))
}

fn sshd() -> Medida {
    match crate::sys::unit_state("sshd").as_str() {
        "active" => Medida::ok("sshd", "serviço sshd rodando"),
        "inactive" => Medida::falta("sshd", "o serviço sshd existe e está parado"),
        outro => Medida::falta("sshd", format!("sshd: {outro}")),
    }
}

fn chave_admin() -> Medida {
    // O OpenSSH do Windows ignora o ~/.ssh/authorized_keys de quem é
    // administrador e lê SÓ este arquivo. Pôr a chave no lugar
    // "óbvio" falha com "Permission denied" e nenhuma pista.
    let base = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
    let p = PathBuf::from(base).join("ssh").join("administrators_authorized_keys");

    // Existir e CONSEGUIR LER são perguntas diferentes aqui, e a
    // diferença me mordeu na primeira medição: o arquivo tem 221 bytes
    // e o wizard anunciou "não achei".
    //
    // A ACL correta deste arquivo é SYSTEM + Administradores e mais
    // ninguém -- o sshd **recusa** o arquivo se for mais frouxa. O
    // daemon sobe por tarefa `/IT` no logon, com o token filtrado pelo
    // UAC, onde o SID de Administradores é deny-only: a leitura falha.
    // (Pelo SSH a mesma conta lê, porque o OpenSSH entrega token
    // cheio. Foi essa assimetria que escondeu o erro.)
    //
    // Então: `metadata` responde "existe" sem precisar ler o conteúdo,
    // e a contagem de chaves só sai quando dá.
    let existe = std::fs::metadata(&p);
    match (existe, std::fs::read_to_string(&p)) {
        (Ok(_), Ok(texto)) => {
            let n = texto.lines().filter(|l| l.trim_start().starts_with("ssh-")).count();
            Medida::se(
                "chave-admin",
                n > 0,
                format!("{n} chave(s) em {}", p.display()),
                format!("{} existe e não tem chave nenhuma", p.display()),
            )
        }
        // Existe e não podemos ler: é o estado ESPERADO numa máquina
        // configurada certo. Marcar falta aqui acusaria defeito justo
        // na configuração correta -- e um wizard que erra no caso bom
        // é um wizard que a pessoa aprende a ignorar. Quem confere o
        // conteúdo de verdade é o passo `pares` das OUTRAS máquinas,
        // que entram por aqui.
        (Ok(m), Err(_)) => Medida::ok(
            "chave-admin",
            format!(
                "existe ({} bytes); só administradores leem, então as chaves não se contam daqui",
                m.len()
            ),
        ),
        (Err(e), _) if e.kind() == std::io::ErrorKind::NotFound => {
            Medida::falta("chave-admin", format!("não achei {}", p.display()))
        }
        (Err(e), _) => Medida::erro("chave-admin", format!("{}: {e}", p.display())),
    }
}

fn protetor_de_tela() -> Medida {
    // Nomes de valor do registro não são traduzidos, a saída do `reg
    // query` é estável, e a pergunta é pequena: o protetor vai entrar e
    // levar a tela remota com ele?
    let chave = "HKCU\\Control Panel\\Desktop";
    let ler = |valor: &str| -> Option<String> {
        let t = rodar("reg", &["query", chave, "/v", valor], 5).ok()?;
        t.lines()
            .find(|l| l.contains(valor))
            .and_then(|l| l.split_whitespace().last())
            .map(str::to_string)
    };
    let ativo = ler("ScreenSaveActive").unwrap_or_else(|| "?".into());
    let seguro = ler("ScreenSaverIsSecure").unwrap_or_else(|| "0".into());
    let tempo: u64 = ler("ScreenSaveTimeOut").and_then(|v| v.parse().ok()).unwrap_or(0);

    if ativo == "0" || tempo == 0 {
        return Medida::ok("protetor-de-tela", "protetor desligado: a tela remota não cai sozinha");
    }
    if seguro == "0" {
        // Protetor sem senha não bloqueia a sessão, e é o bloqueio --
        // não o protetor -- que tira o DXGI do ar.
        return Medida::ok(
            "protetor-de-tela",
            format!("protetor em {}min, sem bloqueio: o DXGI continua", tempo / 60),
        );
    }
    Medida::falta(
        "protetor-de-tela",
        format!("bloqueia em {}min: a tela remota morre com E_ACCESSDENIED", tempo / 60),
    )
}

// ------------------------------------------------------------------ reparos

pub fn arrumar(id: &str, _cfg: &Config) -> Result<String, String> {
    match id {
        "config" => crate::setup_comum::escrever_config(),
        "clipboard" => crate::setup_comum::testar_clipboard(),
        "daemon" => criar_tarefa(
            "daemon",
            "logid",
            &bin_dir().join("logid.exe").to_string_lossy(),
            &["/sc", "onlogon"],
        ),
        // `once` às 00:00 já passou: a tarefa não roda sozinha, só
        // quando alguém pede a tela (`schtasks /run`). O host capturando
        // sem ninguém assistindo seria GPU e energia à toa.
        "tarefa-tela" => criar_tarefa(
            "tarefa-tela",
            "logistream-host",
            &bin_dir().join("logistream-host.exe").to_string_lossy(),
            &["/sc", "once", "/st", "00:00"],
        ),
        "ota" => instalar_relogio(),
        _ => Err(format!("'{id}' não se arruma sozinho nesta máquina")),
    }
}

/// Cria a tarefa de hora em hora e confere uma vez.
///
/// A tarefa roda o `.exe` **sem argumento**. Não é descuido: aqui o
/// `logi-ota` já recusa trocar binário por conta própria (a chave do
/// Authenticode não sai do Ubuntu -- specs/008), então o modo padrão já
/// é "conferir e relatar". E passar argumento no `/tr` exigiria aninhar
/// aspas, que já quebrou esta ponte três vezes.
fn instalar_relogio() -> Result<String, String> {
    let exe = bin_dir().join("logi-ota.exe");
    criar_tarefa("ota", "logi-ota", &exe.to_string_lossy(), &["/sc", "hourly"])?;
    let primeira = rodar(&exe.to_string_lossy(), &["--conferir"], 60);
    Ok(match primeira {
        Ok(s) => format!("tarefa criada; primeira checagem: {}", s.trim()),
        Err(e) => format!("tarefa criada, mas a primeira checagem falhou: {e}"),
    })
}

fn criar_tarefa(id: &str, nome: &str, exe: &str, quando: &[&str]) -> Result<String, String> {
    if !std::path::Path::new(exe).exists() {
        return Err(format!("não achei {exe}: rode o deploy primeiro"));
    }
    let mut args: Vec<&str> = vec!["/create", "/tn", nome, "/tr", exe];
    args.extend_from_slice(quando);
    // `/it` é o ponto inteiro desta função. `/f` sobrescreve uma tarefa
    // anterior -- inclusive uma criada sem `/it`, que é o caso que
    // estamos consertando.
    args.extend_from_slice(&["/it", "/f"]);
    // O exit code é 0 com AVISO quando o /ST está no passado: esperado e
    // desejado. Erro de verdade vem com código != 0, e o `rodar` já o
    // transforma em Err.
    rodar("schtasks", &args, 20)?;
    // Medir depois, não confiar no código: já vi `schtasks` devolver 0
    // criando a tarefa sem o token interativo quando a política da
    // máquina não permite.
    let m = tarefa(id, nome);
    if m.estado == crate::setup::Estado::Ok {
        Ok(m.detalhe)
    } else {
        Err(format!("criei e a conferência diz: {}", m.detalhe))
    }
}
